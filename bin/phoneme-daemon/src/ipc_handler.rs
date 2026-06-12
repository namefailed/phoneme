//! IPC request routing.
//!
//! Each accepted pipe connection runs `handle_connection`, which loops:
//!   1. Read one Request.
//!   2. Call `handle_request` to produce a Response.
//!   3. Send the Response.
//!   4. Repeat until the client closes.
//!
//! `SubscribeEvents` is special — it hijacks the connection for the rest of
//! its life and streams DaemonEvents (wired up in Task 10).

use crate::app_state::AppState;
use phoneme_core::{HookMetadata, HookPayload, HookRunner, RecordingStatus};
use phoneme_ipc::{
    DaemonEvent, IpcError, IpcErrorKind, NamedPipeConnection, PipelineStage, Request, Response,
    ServerRequest,
};

pub async fn handle_connection(mut conn: NamedPipeConnection, state: AppState) {
    loop {
        // Read one request. An unrecognized-but-well-formed request (a client
        // ahead of this daemon during a rolling rebuild) is answered with an
        // error and the connection is KEPT — a single unknown request must never
        // tear down the pipe and break this client's other commands.
        let req = match conn.recv().await {
            Ok(Some(ServerRequest::Known(req))) => *req,
            Ok(Some(ServerRequest::Unknown { detail })) => {
                tracing::warn!(
                    %detail,
                    "unrecognized IPC request; replying with an error and keeping the connection alive"
                );
                let resp = Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!("unsupported or unrecognized request: {detail}"),
                });
                if let Err(e) = conn.send_response(resp).await {
                    tracing::warn!(error = %e, "send_response failed");
                    return;
                }
                continue;
            }
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "recv failed");
                return;
            }
        };
        match req {
            Request::SubscribeEvents => {
                // No ACK Response is sent. The client reframes its connection
                // as a DaemonEvent stream the instant it writes
                // SubscribeEvents — an ACK `Response` would be decoded by that
                // reframed codec as a malformed `DaemonEvent`, abort the
                // stream, and make every blocking `phoneme record` fail. Go
                // straight into event streaming.
                //
                // Backpressure contract: this connection uses a broadcast
                // receiver, which drops old events under lag rather than
                // blocking the producer. On `Lagged(n)`, we tear down the
                // subscription — the client sees the connection close and is
                // expected to reconnect (which freshly re-subscribes) and
                // re-fetch state via `ListRecordings`. Subscribers MUST treat
                // a subscription close as "the world may have moved on; refetch."
                //
                // Closing on lag is preferable to silently dropping events,
                // which would leave the client's incremental UI state diverged
                // from the catalog with no signal that anything's wrong.
                let mut rx = state.events.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            if let Err(e) = conn.send_event(event).await {
                                tracing::debug!(error = %e, "event send failed; subscriber gone");
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                lag = n,
                                "event subscriber lagged; closing subscription so client re-syncs"
                            );
                            return; // client reconnects, re-fetches ListRecordings
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
            other => {
                let response = handle_request(other, &state).await;
                if let Err(e) = conn.send_response(response).await {
                    tracing::warn!(error = %e, "send_response failed");
                    return;
                }
            }
        }
    }
}

pub async fn handle_request(req: Request, state: &AppState) -> Response {
    match req {
        Request::DaemonStatus => Response::Ok(serde_json::json!({
            "running": true,
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
        })),
        Request::RecordStatus => {
            let active = state.recorder.current().await;
            let meeting_active = state.recorder.meeting_active().await;
            let paused = state.recorder.is_paused().await;
            Response::Ok(serde_json::json!({
                "recording": active.is_some(),
                "id": active.as_ref().map(|a| a.id.to_string()),
                "meeting": meeting_active,
                "paused": paused,
            }))
        }
        Request::RecordStart { mode, in_place } => {
            match state.recorder.start(state, mode, in_place).await {
                Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::StartMeeting => match state.recorder.start_meeting(state).await {
            Ok(meeting_id) => Response::Ok(serde_json::json!({ "meeting_id": meeting_id })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::StopMeeting => match state.recorder.stop_meeting(state).await {
            Ok(meeting_id) => Response::Ok(serde_json::json!({ "meeting_id": meeting_id })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::MeetingToggle => {
            // Atomic toggle: the recorder holds a guard across the read+act so a
            // double-tapped hotkey can't race two starts (or two stops). See
            // `DaemonRecorder::toggle_meeting`.
            match state.recorder.toggle_meeting(state).await {
                Ok(started) => Response::Ok(serde_json::json!({ "started": started })),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::RecordStop => match state.recorder.stop(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::RecordToggle { in_place } => {
            if state.recorder.current().await.is_some() {
                match state.recorder.stop(state).await {
                    Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
                    Err(e) => Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: e.to_string(),
                    }),
                }
            } else {
                match state
                    .recorder
                    .start(state, phoneme_core::RecordMode::Hold, in_place)
                    .await
                {
                    Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
                    Err(e) => Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: e.to_string(),
                    }),
                }
            }
        }
        Request::RecordPause => match state.recorder.pause(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::RecordResume => match state.recorder.resume(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::RecordCancel => match state.recorder.cancel(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::ListRecordings { filter } => match state.catalog.list(&filter).await {
            Ok(rows) => serialize_response(rows),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::GetRecording { id } => match state.catalog.get(&id).await {
            Ok(Some(r)) => serialize_response(r),
            Ok(None) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("recording {id} not found"),
            }),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::ListMeeting { meeting_id } => {
            match state.catalog.list_by_meeting(&meeting_id).await {
                Ok(rows) => serialize_response(rows),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        // An unknown id yields an empty list, not NotFound — "no segments" is
        // a normal state (pre-capture recordings, providers without timing)
        // and callers treat the two identically.
        Request::GetSegments { id } => match state.catalog.segments_for(&id).await {
            Ok(segments) => serialize_response(segments),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::SemanticSearch { query, limit } => {
            // Minimum *calibrated* relevance (0..1) a semantic-only hit must clear
            // to surface. Hybrid search ranks by per-chunk best-match cosine fused
            // (RRF) with the FTS5 lexical ranking, so this is no longer a fragile
            // raw-cosine floor that silently dropped good paraphrase matches: a
            // lexical (exact-term) hit is never filtered by this, and the score is
            // calibrated so 0.12 ≈ "barely related". See catalog::hybrid_search.
            const SEMANTIC_MIN_RELEVANCE: f32 = 0.12;
            let embedder_guard = state.embedder.read().await;
            if let Some(embedder) = embedder_guard.as_ref() {
                match embedder.embed_query(&query) {
                    Ok(query_vec) => match state
                        .catalog
                        .hybrid_search(&query, &query_vec, limit, SEMANTIC_MIN_RELEVANCE)
                        .await
                    {
                        Ok(results) => {
                            let mut full_results = Vec::new();
                            for (id, score) in results {
                                if let Ok(Some(r)) = state.catalog.get(&id).await {
                                    full_results.push(serde_json::json!({
                                        "recording": r,
                                        "score": score,
                                    }));
                                }
                            }
                            Response::Ok(serde_json::Value::Array(full_results))
                        }
                        Err(e) => Response::Err(IpcError {
                            kind: error_to_kind(&e),
                            message: e.to_string(),
                        }),
                    },
                    Err(e) => Response::Err(IpcError {
                        kind: IpcErrorKind::Internal,
                        message: format!("embedding failed: {e}"),
                    }),
                }
            } else {
                Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: "Semantic search is not enabled or model is missing.".to_string(),
                })
            }
        }
        Request::ReembedAll => {
            let cfg = state.config.load();
            if !cfg.semantic_search.enabled {
                Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: "semantic search is disabled — enable it before re-embedding".into(),
                })
            } else if state.embedder.read().await.is_none() {
                Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: "embedding model is not loaded (check the model path)".into(),
                })
            } else {
                // Clear every old vector, then re-embed the whole library in the
                // background with the current model. Returns immediately.
                match state.catalog.clear_all_embeddings().await {
                    Ok(()) => {
                        let bg = state.clone();
                        tokio::spawn(async move {
                            let guard = bg.embedder.read().await;
                            let Some(embedder) = guard.as_ref() else {
                                return;
                            };
                            match bg.catalog.list_recordings_without_chunk_embeddings().await {
                                Ok(records) => {
                                    let n = records.len();
                                    tracing::info!(
                                        "re-embedding {n} recordings with the current model"
                                    );
                                    for r in records {
                                        if let Some(t) = r.transcript.as_ref() {
                                            crate::pipeline::embed_and_store(
                                                embedder,
                                                &bg.catalog,
                                                &r.id,
                                                t,
                                            )
                                            .await;
                                        }
                                    }
                                    tracing::info!("re-embed complete ({n} recordings)");
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "re-embed: failed to list recordings")
                                }
                            }
                        });
                        Response::Ok(serde_json::Value::Null)
                    }
                    Err(e) => Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: format!("failed to clear embeddings: {e}"),
                    }),
                }
            }
        }
        Request::DeleteRecording { id, keep_audio } => match state.catalog.get(&id).await {
            Ok(Some(r)) => {
                // Delete the catalog row first. If it fails, report the error
                // and DON'T touch the audio — otherwise the client sees `Ok`,
                // the WAV is gone, and the row lingers pointing at nothing.
                if let Err(e) = state.catalog.delete(&id).await {
                    return Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: format!("catalog delete failed: {e}"),
                    });
                }
                if !keep_audio {
                    // Defense-in-depth: only ever unlink files that live under
                    // our own audio directory. The path comes from the catalog
                    // (which we control), but guarding here means a poisoned or
                    // hand-edited row can't turn a delete into "rm any file".
                    if audio_path_is_ours(&r.audio_path, &state.paths.audio_dir) {
                        // Best-effort — the file may already be gone. Log, don't fail.
                        if let Err(e) = tokio::fs::remove_file(&r.audio_path).await {
                            tracing::warn!(
                                path = %r.audio_path,
                                error = %e,
                                "audio file removal failed"
                            );
                        }
                    } else {
                        tracing::warn!(
                            path = %r.audio_path,
                            "refusing to delete audio file outside the audio directory"
                        );
                    }
                }
                state.events.emit(DaemonEvent::RecordingDeleted { id });
                Response::Ok(serde_json::Value::Null)
            }
            Ok(None) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("recording {id} not found"),
            }),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::UpdateTranscript { id, text } => {
            match state.catalog.update_user_transcript(&id, &text).await {
                Ok(()) => {
                    let embedder_guard = state.embedder.read().await;
                    if let Some(embedder) = embedder_guard.as_ref() {
                        crate::pipeline::embed_and_store(embedder, &state.catalog, &id, &text)
                            .await;
                    }
                    drop(embedder_guard);

                    state.events.emit(DaemonEvent::TranscriptUpdated { id });
                    Response::Ok(serde_json::Value::Null)
                }
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::UpdateMeetingName { meeting_id, name } => {
            match state
                .catalog
                .update_meeting_name(&meeting_id, name.as_deref())
                .await
            {
                Ok(()) => {
                    state
                        .events
                        .emit(DaemonEvent::MeetingNameUpdated { meeting_id });
                    Response::Ok(serde_json::Value::Null)
                }
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::GetOriginalTranscript { id } => {
            match state.catalog.get_original_transcript(&id).await {
                Ok(original) => serialize_response(original),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::GetCleanTranscript { id } => match state.catalog.get_clean_transcript(&id).await {
            Ok(clean) => serialize_response(clean),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::SetFavorite { id, favorite } => {
            match state.catalog.set_favorite(&id, favorite).await {
                Ok(()) => Response::Ok(serde_json::Value::Null),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::SuggestTags { id } => {
            // On-demand tag suggestions (the UI's ✨ Suggest button). Runs the
            // same step as the auto pipeline, regardless of `auto_tag.auto`.
            let cfg = state.config.load();
            match state.catalog.get(&id).await {
                Ok(Some(rec)) => {
                    let transcript = rec.transcript.unwrap_or_default();
                    if transcript.trim().is_empty() {
                        Response::Err(IpcError {
                            kind: IpcErrorKind::InvalidConfig,
                            message: "recording has no transcript to tag yet".into(),
                        })
                    } else {
                        crate::pipeline::suggest_tags(state, &cfg, &id, &transcript).await;
                        Response::Ok(serde_json::Value::Null)
                    }
                }
                Ok(None) => Response::Err(IpcError {
                    kind: IpcErrorKind::NotFound,
                    message: format!("no recording {}", id.as_str()),
                }),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::ApproveTagSuggestion { id, name } => {
            // Create-or-fetch the tag, attach it, then drop the suggestion.
            match state.catalog.add_tag(&name, None).await {
                Ok(tag) => match state.catalog.attach_tag(&id, tag.id).await {
                    Ok(()) => {
                        state
                            .events
                            .emit(DaemonEvent::TagAttached { tag_id: tag.id });
                        if let Ok(Some(rec)) = state.catalog.get(&id).await {
                            let rest: Vec<String> = rec
                                .tag_suggestions
                                .into_iter()
                                .filter(|n| !n.eq_ignore_ascii_case(&name))
                                .collect();
                            if let Err(e) = state.catalog.set_tag_suggestions(&id, &rest).await {
                                tracing::warn!(error = %e, "failed to drop approved tag suggestion");
                            }
                        }
                        state.events.emit(DaemonEvent::TagSuggestionsUpdated { id });
                        Response::Ok(serde_json::to_value(tag).unwrap_or_default())
                    }
                    Err(e) => Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: e.to_string(),
                    }),
                },
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::ClearAllTagSuggestions => match state.catalog.clear_all_tag_suggestions().await {
            Ok(cleared) => {
                state
                    .events
                    .emit(DaemonEvent::AllTagSuggestionsCleared { cleared });
                Response::Ok(serde_json::json!({ "cleared": cleared }))
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::DismissTagSuggestion { id, name } => match state.catalog.get(&id).await {
            Ok(Some(rec)) => {
                let rest: Vec<String> = rec
                    .tag_suggestions
                    .into_iter()
                    .filter(|n| !n.eq_ignore_ascii_case(&name))
                    .collect();
                match state.catalog.set_tag_suggestions(&id, &rest).await {
                    Ok(()) => {
                        state.events.emit(DaemonEvent::TagSuggestionsUpdated { id });
                        Response::Ok(serde_json::Value::Null)
                    }
                    Err(e) => Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: e.to_string(),
                    }),
                }
            }
            Ok(None) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("no recording {}", id.as_str()),
            }),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::UpdateNotes { id, notes } => match state.catalog.update_notes(&id, &notes).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::NotesUpdated { id });
                Response::Ok(serde_json::Value::Null)
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::SetSpeakerName {
            id,
            speaker_label,
            name,
        } => {
            // Speaker indices are 1-based (`[Speaker 1]`, …); reject a non-positive
            // label rather than writing a row that can never match a marker.
            if speaker_label < 1 {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!("invalid speaker label {speaker_label} (must be >= 1)"),
                });
            }
            match state
                .catalog
                .set_speaker_name(&id, speaker_label, &name)
                .await
            {
                Ok(()) => {
                    state.events.emit(DaemonEvent::SpeakerNameUpdated { id });
                    Response::Ok(serde_json::Value::Null)
                }
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::ImportRecording { path } => import_recording(state, path).await,
        Request::RetranscribeRecording {
            id,
            model,
            run_hooks,
            post_process,
            all_overrides,
        } => match state.catalog.get(&id).await {
            Ok(Some(r)) => {
                let mut cfg = state.config.load().as_ref().clone();
                let mut changed = false;
                // A per-recording model override is NO LONGER written into the
                // process-global config. Doing so made the whisper supervisor
                // (which polls the global config) restart the server, and the
                // queue worker's blanket post-run reload restart it again — a
                // thrash that mass-failed other queued/preview jobs reading the
                // same global config (#49). Instead we record the requested model
                // against this recording id; the pipeline applies it to that one
                // job only (a single serialized server model-swap for the local
                // bundled backend, or a per-job config clone for cloud backends),
                // then restores. See `pipeline::run`.
                if let Some(m) = model {
                    let m = m.trim();
                    if m.is_empty() {
                        // Empty = "use the configured model"; clear any stale
                        // request so a prior override can't leak onto this run.
                        state.pending_overrides.lock().unwrap().remove(&id);
                    } else {
                        state
                            .pending_overrides
                            .lock()
                            .unwrap()
                            .insert(id.clone(), m.to_string());
                    }
                }
                if let Some(rh) = run_hooks {
                    cfg.hook.run_on_transcribe = rh;
                    changed = true;
                }
                // One-time post-processing opt-out: disabling cleanup in this
                // temporary in-memory config makes the pipeline's
                // `llm.provider(...)` return `None`, so the run yields the raw
                // machine transcript. The queue worker reloads config from disk
                // after the job, so this never persists (the configured cleanup
                // behavior is restored for the next recording).
                if post_process == Some(false) {
                    cfg.llm_post_process.enabled = false;
                    changed = true;
                }
                // Re-run → "All": force the whole pipeline on and layer the
                // one-time cleanup + summary overrides into the temp config.
                // (Applied after the post_process opt-out so cleanup stays on.)
                if let Some(ov) = all_overrides {
                    cfg.llm_post_process.enabled = true;
                    if let Some(p) = ov.cleanup_provider {
                        cfg.llm_post_process.provider = p;
                    }
                    if let Some(m) = ov.cleanup_model {
                        cfg.llm_post_process.model = m;
                    }
                    if let Some(p) = ov.cleanup_prompt {
                        cfg.llm_post_process.prompt = p;
                    }
                    if let Some(u) = ov.cleanup_api_url {
                        cfg.llm_post_process.api_url = u;
                    }
                    cfg.summary.auto = true;
                    if let Some(m) = ov.summary_model {
                        cfg.summary.model = m;
                    }
                    if let Some(p) = ov.summary_prompt {
                        cfg.summary.prompt = p;
                    }
                    changed = true;
                }
                if changed {
                    // NOTE: this temp-global config carries only server-independent
                    // overrides (hooks / cleanup / summary). The whisper model is
                    // deliberately NOT here — see the per-recording override above.
                    state.config.store(std::sync::Arc::new(cfg));
                }
                let payload = HookPayload {
                    id: r.id,
                    timestamp: r.started_at,
                    transcript: String::new(),
                    audio_path: r.audio_path,
                    duration_ms: r.duration_ms,
                    model: String::new(),
                    metadata: HookMetadata::current(),
                };
                match state.inbox.enqueue(&payload).await {
                    Ok(()) => {
                        if let Err(e) = state
                            .catalog
                            .update_status(&id, RecordingStatus::Transcribing)
                            .await
                        {
                            tracing::error!("failed to update status to transcribing: {e}");
                        }
                        Response::Ok(serde_json::Value::Null)
                    }
                    Err(e) => Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: e.to_string(),
                    }),
                }
            }
            Ok(None) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("recording {id} not found"),
            }),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::RefireHook { id, command } => match state.catalog.get(&id).await {
            Ok(Some(r)) if r.transcript.is_some() => {
                let payload = HookPayload {
                    id: r.id,
                    timestamp: r.started_at,
                    transcript: r.transcript.unwrap_or_default(),
                    audio_path: r.audio_path,
                    duration_ms: r.duration_ms,
                    model: r.model.unwrap_or_default(),
                    metadata: HookMetadata::current(),
                };
                let cfg = state.config.load();
                let timeout = std::time::Duration::from_secs(cfg.hook.timeout_secs);
                let configured = match cfg.expanded() {
                    Ok(c) => c.hook.commands,
                    Err(_) => cfg.hook.commands.clone(),
                };
                drop(cfg);
                let commands = if let Some(cmd) = command {
                    // Security (S-C2): a caller may only re-fire a command that is
                    // already in the configured hook allowlist — never an arbitrary
                    // command handed in over IPC. The UI only ever sends a command
                    // it picked from this same list, so legitimate flows are intact.
                    if !hook_command_allowed(&cmd, &configured) {
                        return Response::Err(IpcError {
                            kind: IpcErrorKind::Internal,
                            message: "refire command is not in the configured hook allowlist"
                                .into(),
                        });
                    }
                    vec![cmd]
                } else {
                    configured
                };
                // Run the hook OFF the IPC connection. A hook can take up to
                // its full timeout (30s default); running it inline froze the
                // connection — and with it the single-connection Tauri bridge,
                // stalling every other UI request. The outcome is reported via
                // DaemonEvents, exactly like the queue pipeline.
                //
                // We deliberately do NOT re-enqueue (as RetranscribeRecording does):
                // the queue pipeline always re-transcribes first, which would
                // overwrite a user's manual transcript edit. RefireHook must
                // re-run only the hook against the stored transcript.
                let task_state = state.clone();
                tokio::spawn(async move {
                    let hook_id = payload.id.clone();
                    task_state.events.emit(DaemonEvent::HookStarted {
                        id: hook_id.clone(),
                    });
                    // Surface this re-run in the queue as an active "Running hook…" item.
                    task_state.events.emit(DaemonEvent::PipelineStageChanged {
                        id: hook_id.clone(),
                        stage: PipelineStage::RunningHook,
                    });

                    // Mirror pipeline::run: execute every configured hook in
                    // order, stopping at the first non-zero exit, and record the
                    // last command that ran.
                    let mut final_exit_code = 0;
                    let mut total_duration = 0;
                    let mut last_cmd = String::new();
                    let mut hook_error: Option<String> = None;
                    for cmd in &commands {
                        let trimmed = cmd.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let runner = HookRunner::new(trimmed.to_string(), timeout);
                        match runner.run(&payload).await {
                            Ok(result) => {
                                final_exit_code = result.exit_code;
                                total_duration += result.duration_ms;
                                last_cmd = cmd.clone();
                                if result.exit_code != 0 {
                                    break;
                                }
                            }
                            Err(e) => {
                                hook_error = Some(e.to_string());
                                break;
                            }
                        }
                    }

                    if let Some(error) = hook_error {
                        if let Err(e) = task_state
                            .catalog
                            .update_status(&hook_id, RecordingStatus::HookFailed)
                            .await
                        {
                            tracing::error!("failed to update status to hook_failed: {e}");
                        }
                        task_state
                            .events
                            .emit(DaemonEvent::HookFailed { id: hook_id, error });
                    } else {
                        if let Err(e) = task_state
                            .catalog
                            .update_hook_result(
                                &hook_id,
                                &last_cmd,
                                final_exit_code,
                                total_duration,
                            )
                            .await
                        {
                            tracing::error!("failed to update hook result: {e}");
                        }
                        if let Err(e) = task_state
                            .catalog
                            .update_status(&hook_id, RecordingStatus::Done)
                            .await
                        {
                            tracing::error!("failed to update status to done: {e}");
                        }
                        task_state.events.emit(DaemonEvent::HookDone {
                            id: hook_id,
                            exit_code: final_exit_code,
                        });
                    }
                });
                Response::Ok(serde_json::Value::Null)
            }
            Ok(Some(_)) => Response::Err(IpcError {
                kind: IpcErrorKind::Internal,
                message: "no transcript to fire hook against".into(),
            }),
            Ok(None) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("recording {id} not found"),
            }),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::RerunCleanup {
            id,
            model,
            provider,
            prompt,
            api_url,
            api_key,
        } => {
            rerun_cleanup(
                state,
                id,
                CleanupOverrides {
                    model,
                    provider,
                    prompt,
                    api_url,
                    api_key,
                },
            )
            .await
        }
        Request::RerunSummary { id, model, prompt } => {
            rerun_summary(state, id, model, prompt).await
        }
        Request::RunDoctor => {
            let cfg = state.config.load();
            let mut checks = phoneme_core::doctor::run_local_checks(&cfg);
            checks.extend(phoneme_core::doctor::run_backend_checks(&cfg).await);
            serialize_response(checks)
        }
        Request::RestartWhisper => {
            // Sweep every whisper-server process (hung children AND orphans
            // from a dead daemon still holding the port), then wake both
            // supervisors so the main + preview servers respawn from config.
            crate::whisper_supervisor::sweep_stray_servers();
            state.whisper_restart.notify_waiters();
            tracing::info!("whisper-server restart requested via IPC (Doctor fix)");
            Response::Ok(serde_json::json!({
                "message": "whisper-server processes swept; supervisors respawning"
            }))
        }
        Request::SetPreviewSource { track } => {
            match state.recorder.set_preview_source(state, &track).await {
                Ok(()) => Response::Ok(serde_json::Value::Null),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::SkipCurrentStage => {
            // Wakes whichever LLM stage is currently streaming (no-op when none
            // is — the notify has no waiter then and stores nothing).
            state.skip_stage.notify_waiters();
            tracing::info!("skip-current-stage requested via IPC");
            Response::Ok(serde_json::Value::Null)
        }
        Request::ListQueue => {
            let pending = state.inbox.list_pending().await;
            let processing = state.inbox.list_processing().await;
            match (pending, processing) {
                (Ok(pending), Ok(processing)) => {
                    let entry = |p: &phoneme_core::HookPayload, queue_state: &str| {
                        serde_json::json!({
                            "id": p.id,
                            "timestamp": p.timestamp,
                            "audio_path": p.audio_path,
                            "duration_ms": p.duration_ms,
                            "model": p.model,
                            "state": queue_state,
                        })
                    };
                    let mut items: Vec<serde_json::Value> = Vec::new();
                    // The actively-processing item(s) first, then the pending queue.
                    items.extend(processing.iter().map(|p| entry(p, "processing")));
                    items.extend(pending.iter().map(|p| entry(p, "pending")));
                    Response::Ok(serde_json::Value::Array(items))
                }
                (Err(e), _) | (_, Err(e)) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::ReorderQueue { ids } => match state.inbox.set_order(&ids).await {
            Ok(()) => {
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::Value::Null)
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::CancelQueued { id } => match state.inbox.cancel_pending(&id).await {
            Ok(true) => {
                // Leave the recording in a terminal state so it isn't stuck
                // showing "transcribing"; the user can re-run it later.
                let _ = state
                    .catalog
                    .update_status(&id, RecordingStatus::TranscribeFailed)
                    .await;
                state
                    .events
                    .emit(DaemonEvent::RecordingCancelled { id: id.clone() });
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::Value::Null)
            }
            Ok(false) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: "recording is not in the pending queue (already processing or finished)"
                    .into(),
            }),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::SetQueuePaused { paused } => match state.inbox.set_paused(paused).await {
            Ok(()) => {
                // Nudge the panel so the pause state reflects immediately.
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "paused": paused }))
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::QueuePaused => {
            Response::Ok(serde_json::json!({ "paused": state.inbox.is_paused().await }))
        }
        Request::QueueCounts => match state.inbox.counts().await {
            Ok(c) => Response::Ok(serde_json::json!({
                "pending": c.pending,
                "processing": c.processing,
                "done": c.done,
                "failed": c.failed,
            })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::ClearFailed => match state.inbox.clear_failed().await {
            Ok(removed) => {
                // Refresh the depth so the panel's failed badge clears at once.
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "removed": removed }))
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::CancelProcessing { id } => {
            // Signal the in-flight cancellation token only if `id` is the item
            // currently processing; the worker + pipeline finalize the rest.
            let canceled = {
                match state.processing.lock() {
                    Ok(slot) => match slot.as_ref() {
                        Some((pid, token)) if *pid == id => {
                            token.cancel();
                            true
                        }
                        _ => false,
                    },
                    Err(_) => false,
                }
            };
            if canceled {
                Response::Ok(serde_json::Value::Null)
            } else {
                Response::Err(IpcError {
                    kind: IpcErrorKind::NotFound,
                    message: "recording is not the item currently being processed".into(),
                })
            }
        }
        Request::CancelAllQueued => match state.inbox.cancel_all_pending().await {
            Ok(ids) => {
                // Mark each cancelled recording terminal so it isn't stuck
                // showing "transcribing", mirroring single-item CancelQueued.
                for id in &ids {
                    let _ = state
                        .catalog
                        .update_status(id, RecordingStatus::TranscribeFailed)
                        .await;
                    state
                        .events
                        .emit(DaemonEvent::RecordingCancelled { id: id.clone() });
                }
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "removed": ids.len() }))
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        // Unlike RefireHook, HookTest intentionally runs a caller-supplied
        // command: it is the Hook Manager's "test this command" affordance, used
        // to validate a hook the user is editing but has not saved yet. That is a
        // deliberate, user-initiated test — gated by the owner-only IPC pipe
        // (S-C1) — so it is not an additional privilege-escalation channel and is
        // not subject to the RefireHook allowlist (S-C2).
        Request::HookTest { custom_command } => {
            let command = custom_command.unwrap_or_else(|| {
                state
                    .config
                    .load()
                    .hook
                    .commands
                    .first()
                    .cloned()
                    .unwrap_or_default()
            });
            let runner = HookRunner::new(
                command,
                std::time::Duration::from_secs(state.config.load().hook.timeout_secs),
            );
            // Build a representative test payload. Use a plausible-looking
            // audio path so hooks that reference it (e.g. file-logging hooks)
            // receive a non-empty string rather than silently writing nothing.
            let placeholder_audio = {
                let base = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .unwrap_or_else(|_| String::from("C:\\Users\\user"));
                format!("{base}\\Documents\\phoneme\\audio\\test\\sample.wav")
            };
            let sample = HookPayload {
                id: phoneme_core::RecordingId::new(),
                timestamp: chrono::Local::now(),
                transcript: "This is a test transcript for the hook.".into(),
                audio_path: placeholder_audio,
                duration_ms: 3500,
                model: "test".into(),
                metadata: HookMetadata::current(),
            };
            match runner.run(&sample).await {
                Ok(result) => Response::Ok(serde_json::json!({
                    "exit_code": result.exit_code,
                    "duration_ms": result.duration_ms,
                    "stderr_tail": result.stderr_tail,
                })),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::Shutdown => {
            tracing::info!("shutdown requested via IPC");
            // Trigger the shared coordinator `main` waits on, so
            // `phoneme daemon stop` actually stops the daemon.
            state.shutdown.trigger();
            Response::Ok(serde_json::Value::Null)
        }
        Request::ListTags => match state.catalog.list_tags().await {
            Ok(tags) => Response::Ok(serde_json::to_value(tags).unwrap_or_default()),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::ListAllTags => match state.catalog.list_all_tags().await {
            Ok(tags) => Response::Ok(serde_json::to_value(tags).unwrap_or_default()),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::AddTag { name, color } => {
            match state.catalog.add_tag(&name, color.as_deref()).await {
                Ok(tag) => {
                    state.events.emit(DaemonEvent::TagCreated { id: tag.id });
                    Response::Ok(serde_json::to_value(tag).unwrap_or_default())
                }
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::UpdateTag { id, name, color } => {
            match state.catalog.update_tag(id, &name, color.as_deref()).await {
                Ok(tag) => {
                    state.events.emit(DaemonEvent::TagUpdated { id });
                    Response::Ok(serde_json::to_value(tag).unwrap_or_default())
                }
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::DeleteTag { id } => match state.catalog.delete_tag(id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagDeleted { id });
                Response::Ok(serde_json::Value::Null)
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::AttachTag {
            recording_id,
            tag_id,
        } => match state.catalog.attach_tag(&recording_id, tag_id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagAttached { tag_id });
                Response::Ok(serde_json::Value::Null)
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::DetachTag {
            recording_id,
            tag_id,
        } => match state.catalog.detach_tag(&recording_id, tag_id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagDetached { tag_id });
                Response::Ok(serde_json::Value::Null)
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::TagsFor { recording_id } => match state.catalog.tags_for(&recording_id).await {
            Ok(tags) => Response::Ok(serde_json::to_value(tags).unwrap_or_default()),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::TagUsageCounts => match state.catalog.tag_usage_counts().await {
            Ok(counts) => Response::Ok(serde_json::to_value(counts).unwrap_or_default()),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::MergeTags { from_id, into_id } => {
            match state.catalog.merge_tags(from_id, into_id).await {
                Ok(()) => {
                    // The source tag is gone; consumers refresh on TagDeleted.
                    state.events.emit(DaemonEvent::TagDeleted { id: from_id });
                    Response::Ok(serde_json::Value::Null)
                }
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::ReloadConfig => {
            tracing::info!("reloading config via IPC");
            match crate::load_config() {
                Ok(cfg) => {
                    state.config.store(std::sync::Arc::new(cfg));

                    let cfg_arc = state.config.load();
                    let mut embedder_guard = state.embedder.write().await;
                    if cfg_arc.semantic_search.enabled {
                        // (Re)build on every reload so a changed model_dir /
                        // pooling / max_tokens / prefix actually takes effect on
                        // save — not only when no model was loaded before. On
                        // failure keep the previous model so search doesn't break.
                        match phoneme_core::Embedder::new(&cfg_arc.semantic_search) {
                            Ok(e) => *embedder_guard = Some(std::sync::Arc::new(e)),
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to (re)load semantic search model on reload; keeping the previous one")
                            }
                        }
                    } else {
                        *embedder_guard = None;
                    }
                    drop(cfg_arc);
                    drop(embedder_guard);

                    // Start/stop idle pre-roll pre-capture to match the new
                    // config (e.g. user just toggled pre_roll_ms).
                    state.recorder.sync_preroll(state).await;
                    Response::Ok(serde_json::Value::Null)
                }
                Err(e) => Response::Err(IpcError {
                    kind: IpcErrorKind::InvalidConfig,
                    message: format!("failed to load config: {e}"),
                }),
            }
        }
        Request::SubscribeEvents => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message:
                "subscribe_events is handled by the streaming path in handle_connection (Task 10)"
                    .into(),
        }),
    }
}

/// Hard cap on the on-disk size of an importable file. The Tauri file dialog is
/// the intended sole producer, but `ImportRecording` accepts an arbitrary client
/// path — this bounds a bypass that could otherwise feed the decoder a
/// pathologically large file and exhaust memory (the decoder buffers the whole
/// file into a single `Vec<f32>`; see `phoneme-audio::decode`). 2 GiB is far
/// beyond any realistic voice note while still leaving the decode duration cap
/// (in `phoneme-audio`) as the real memory bound.
const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Returns `true` if an on-disk file of `len` bytes exceeds the import size cap.
/// Factored out so the bound is unit-testable without a multi-GiB fixture file.
fn exceeds_import_size_cap(len: u64) -> bool {
    len > MAX_IMPORT_BYTES
}

/// Whether `requested` matches a configured hook command (compared trimmed).
///
/// The IPC `RefireHook` request lets a caller pass a command to run; without
/// this check any process reaching the pipe could run an arbitrary command via
/// the daemon. Restricting to the already-configured hooks turns it into "re-run
/// one of my hooks" instead of an open exec channel. (audit S-C2)
fn hook_command_allowed(requested: &str, configured: &[String]) -> bool {
    let requested = requested.trim();
    !requested.is_empty() && configured.iter().any(|c| c.trim() == requested)
}

/// Returns `true` if `audio_path` is a normal path located under `audio_dir`.
///
/// The path comes from our own catalog, so this is defense-in-depth: we reject
/// any `..` component (which could climb out of the audio directory) and require
/// the rest to be prefixed by `audio_dir` component-wise. Kept purely lexical so
/// it is unit-testable without touching the filesystem and never deletes the
/// wrong file just because canonicalization of an already-removed file failed.
fn audio_path_is_ours(audio_path: &str, audio_dir: &std::path::Path) -> bool {
    use std::path::Component;
    let p = std::path::Path::new(audio_path);
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    p.starts_with(audio_dir)
}

/// Re-run ONLY the LLM post-processing ("cleanup") step on a recording's
/// already-stored transcript, without re-transcribing the audio.
///
/// Design mirrors `RefireHook`: validate up front on the IPC connection (the
/// recording must exist and have a transcript), then do the slow work — the LLM
/// call, which can take its full timeout — OFF the connection in a spawned task,
/// reporting progress via the same `DaemonEvent`s the UI already consumes. This
/// keeps the single-connection Tauri bridge responsive.
///
/// Input baseline: the preserved **original** (raw Whisper) transcript when one
/// exists, falling back to the live transcript for recordings predating that
/// column. Running cleanup against the original — not the already-cleaned live
/// text — keeps the operation idempotent (re-running with a different model
/// re-cleans the same source rather than compounding edits) and lets us reuse
/// `update_transcript`, which re-asserts the original alongside the new live
/// text. The original column is therefore preserved by construction.
///
/// An optional `model` overrides the configured cleanup model for THIS run only;
/// it is never written back to config (unlike `RetranscribeRecording`, which
/// must restart the whisper server). The post-processing provider is built from
/// a cloned config with just the model field swapped.
/// One-time, per-run overrides for [`rerun_cleanup`]. Each field falls back to
/// the configured `[llm_post_process]` value when `None` and is never persisted.
#[derive(Default)]
struct CleanupOverrides {
    model: Option<String>,
    provider: Option<String>,
    prompt: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
}

async fn rerun_cleanup(
    state: &AppState,
    id: phoneme_core::RecordingId,
    overrides: CleanupOverrides,
) -> Response {
    let recording = match state.catalog.get(&id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("recording {id} not found"),
            });
        }
        Err(e) => {
            return Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            });
        }
    };

    // Cleanup operates on text — there must be something to clean.
    if recording.transcript.is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "no transcript to run cleanup on".into(),
        });
    }

    // Build a cleanup config with the optional one-time overrides applied.
    // Cloning the live config and swapping only the supplied fields keeps every
    // other setting intact and — crucially — never persists the override the way
    // RetranscribeRecording does (this config is local to the spawned task).
    let CleanupOverrides {
        model,
        provider,
        prompt,
        api_url,
        api_key,
    } = overrides;
    let mut llm_cfg = state.config.load().llm_post_process.clone();
    // Override the provider for this run, but do NOT force the step on: if
    // post-processing is disabled in config, cleanup stays unavailable ("off
    // means off"). The validation below reports that clearly, and the GUI
    // disables the Cleanup option entirely when cleanup is off.
    if let Some(p) = provider {
        let p = p.trim();
        if !p.is_empty() {
            llm_cfg.provider = p.to_string();
        }
    }
    if let Some(m) = model {
        let m = m.trim();
        if !m.is_empty() {
            llm_cfg.model = m.to_string();
        }
    }
    // A blank prompt would strip the cleanup instructions, so keep the
    // configured prompt unless a non-empty override is given.
    if let Some(pr) = prompt {
        if !pr.trim().is_empty() {
            llm_cfg.prompt = pr;
        }
    }
    // An explicit empty URL is meaningful (= "use the provider default"), so
    // honor any provided value rather than only non-empty ones.
    if let Some(u) = api_url {
        llm_cfg.api_url = u;
    }
    if let Some(k) = api_key {
        let k = k.trim();
        if !k.is_empty() {
            llm_cfg.set_api_key(k.to_string());
        }
    }

    // Require post-processing to actually be configured. `provider()` returns
    // None when disabled or the provider is `none`/unrecognized — in that case
    // there is nothing to run, so report it rather than silently no-op'ing.
    if state.llm.provider(&llm_cfg).is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::InvalidConfig,
            message: "LLM post-processing is not enabled (set [llm_post_process] provider)".into(),
        });
    }

    // Choose the cleanup INPUT: prefer the preserved original (raw machine
    // output) so cleanup is idempotent; fall back to the current transcript for
    // older rows that have no original stored.
    let source = match state.catalog.get_original_transcript(&id).await {
        Ok(Some(original)) if !original.is_empty() => original,
        // No original (or empty): fall back to the live transcript. Safe to
        // unwrap — we returned above if it was None.
        _ => recording.transcript.clone().unwrap_or_default(),
    };

    let task_state = state.clone();
    tokio::spawn(async move {
        // Re-build the provider inside the task from the (already-validated)
        // config so the heavy work — the network call to the LLM — happens off
        // the IPC connection. We re-check `provider()` only to obtain the boxed
        // provider; the None branch is unreachable in practice but handled
        // defensively rather than unwrapped.
        let Some(provider) = task_state.llm.provider(&llm_cfg) else {
            return;
        };

        // Surface this re-run in the queue as an active "Cleaning up…" item.
        task_state.events.emit(DaemonEvent::PipelineStageChanged {
            id: id.clone(),
            stage: PipelineStage::CleaningUp,
        });

        match crate::pipeline::run_llm_stage(
            &task_state,
            &id,
            PipelineStage::CleaningUp,
            &*provider,
            &llm_cfg.prompt,
            &source,
        )
        .await
        {
            Ok(cleaned) => {
                // Re-assert the original alongside the freshly cleaned live text.
                // Reusing `update_transcript` (the same call the pipeline makes)
                // keeps `original_transcript` pinned to the raw source we cleaned.
                if let Err(e) = task_state
                    .catalog
                    .update_transcript(&id, &cleaned, &source, &llm_cfg.model)
                    .await
                {
                    tracing::error!(error = %e, "rerun_cleanup: failed to update transcript");
                    task_state.events.emit(DaemonEvent::TranscriptionFailed {
                        id,
                        error: e.to_string(),
                    });
                    return;
                }
                // Record which cleanup model ran (diarization state is unchanged
                // by a text-only re-clean, so preserve whatever was stored).
                if let Err(e) = task_state
                    .catalog
                    .update_processing_meta(&id, Some(&llm_cfg.model), recording.diarized)
                    .await
                {
                    tracing::warn!(error = %e, "rerun_cleanup: failed to update processing meta");
                }

                // Re-embed the new text so semantic search stays consistent,
                // mirroring the pipeline and UpdateTranscript paths.
                let embedder_guard = task_state.embedder.read().await;
                if let Some(embedder) = embedder_guard.as_ref() {
                    crate::pipeline::embed_and_store(embedder, &task_state.catalog, &id, &cleaned)
                        .await;
                }
                drop(embedder_guard);

                // Emit the same event the UI already listens for after a manual
                // transcript change so the detail/list views refresh in place.
                task_state
                    .events
                    .emit(DaemonEvent::TranscriptUpdated { id });
            }
            Err(e) => {
                tracing::error!(error = %e, "rerun_cleanup: LLM post-processing failed");
                task_state.events.emit(DaemonEvent::TranscriptionFailed {
                    id,
                    error: e.to_string(),
                });
            }
        }
    });

    Response::Ok(serde_json::Value::Null)
}

/// Generate (or regenerate) an LLM summary of a recording's current transcript
/// on demand. Like `rerun_cleanup`, the network call runs in a spawned task so
/// it doesn't block the IPC connection; the UI listens for `SummaryUpdated`.
/// `model`/`prompt` override the configured summary model/prompt for this run
/// only and are never persisted.
async fn rerun_summary(
    state: &AppState,
    id: phoneme_core::RecordingId,
    model: Option<String>,
    prompt: Option<String>,
) -> Response {
    let recording = match state.catalog.get(&id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("recording {id} not found"),
            });
        }
        Err(e) => {
            return Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            });
        }
    };

    let transcript = recording.transcript.clone().unwrap_or_default();
    if transcript.trim().is_empty() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "no transcript to summarize".into(),
        });
    }

    // Clone the live config and apply the one-time summary overrides. Summaries
    // reuse the [llm_post_process] provider connection; only model/prompt are
    // summary-specific.
    let mut cfg = (**state.config.load()).clone();
    if let Some(m) = model {
        if !m.trim().is_empty() {
            cfg.summary.model = m;
        }
    }
    if let Some(p) = prompt {
        if !p.trim().is_empty() {
            cfg.summary.prompt = p;
        }
    }

    // Require a usable LLM provider up front so the user gets a clear error
    // rather than a silent no-op. `generate_summary` re-checks defensively.
    {
        let probe = crate::pipeline::summary_llm_config(&cfg);
        if state.llm.provider(&probe).is_none() {
            return Response::Err(IpcError {
                kind: IpcErrorKind::InvalidConfig,
                message: "no LLM provider configured for summaries (set a summary or [llm_post_process] provider)"
                    .into(),
            });
        }
    }

    let task_state = state.clone();
    tokio::spawn(async move {
        // Surface this re-run in the queue as an active "Summarizing…" item.
        task_state.events.emit(DaemonEvent::PipelineStageChanged {
            id: id.clone(),
            stage: PipelineStage::Summarizing,
        });
        match crate::pipeline::generate_summary(&task_state, &cfg, &id, &transcript).await {
            Ok((summary, model)) => {
                if let Err(e) = task_state
                    .catalog
                    .update_summary(&id, &summary, Some(&model))
                    .await
                {
                    tracing::error!(error = %e, "rerun_summary: failed to persist summary");
                    task_state.events.emit(DaemonEvent::SummaryFailed {
                        id,
                        error: e.to_string(),
                    });
                    return;
                }
                task_state.events.emit(DaemonEvent::SummaryUpdated { id });
            }
            Err(reason) => {
                task_state
                    .events
                    .emit(DaemonEvent::SummaryFailed { id, error: reason });
            }
        }
    });

    Response::Ok(serde_json::Value::Null)
}

/// Import an existing audio file: decode it to a canonical WAV under the audio
/// dir, insert a catalog row, and enqueue it for the normal transcription
/// pipeline. Mirrors `DaemonRecorder::stop` (catalog row at `Transcribing` +
/// `inbox.enqueue`) so an imported file is processed exactly like a mic
/// recording — the only difference is where the WAV came from.
async fn import_recording(state: &AppState, path: String) -> Response {
    let requested = std::path::PathBuf::from(&path);

    // Canonicalize so the path we open is a fully-resolved, real filesystem
    // location (resolves `..`, symlinks, and relative components). The dialog
    // hands us absolute paths already; this hardens the arbitrary-client-path
    // bypass by ensuring we never act on a half-resolved or traversal path.
    // This inherently checks existence atomically, preventing TOCTOU issues.
    let input = match std::fs::canonicalize(&requested) {
        Ok(p) => p,
        Err(e) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("could not resolve path {path}: {e}"),
            });
        }
    };

    if !phoneme_audio::is_supported_extension(&input) {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: format!(
                "unsupported audio format (supported: {})",
                phoneme_audio::SUPPORTED_EXTENSIONS.join(", ")
            ),
        });
    }

    // Reject oversized files up front via metadata, before decoding allocates
    // anything. Doubles as the coarse memory bound for the import path.
    match std::fs::metadata(&input) {
        Ok(meta) => {
            if !meta.is_file() {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!("not a regular file: {path}"),
                });
            }
            if exceeds_import_size_cap(meta.len()) {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!(
                        "file too large to import ({} bytes; max {} bytes / {} GiB)",
                        meta.len(),
                        MAX_IMPORT_BYTES,
                        MAX_IMPORT_BYTES / (1024 * 1024 * 1024)
                    ),
                });
            }
        }
        Err(e) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::Io,
                message: format!("could not stat {path}: {e}"),
            });
        }
    }

    let id = phoneme_core::RecordingId::new();
    let started_at = chrono::Local::now();
    let audio_path = state
        .paths
        .audio_dir
        .join(id.day_folder())
        .join(format!("{}.wav", id.file_stem()));

    // Decode is CPU-bound and blocking — run it off the async runtime so the
    // IPC connection (and the single-connection Tauri bridge) stays responsive.
    let decode_out = audio_path.clone();
    let decode_result = tokio::task::spawn_blocking(move || {
        phoneme_audio::decode_to_canonical_wav(&input, &decode_out)
    })
    .await;
    let duration_ms = match decode_result {
        Ok(Ok(ms)) => ms,
        Ok(Err(e)) => {
            return Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: format!("failed to decode audio: {e}"),
            });
        }
        Err(e) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::Internal,
                message: format!("decode task panicked: {e}"),
            });
        }
    };

    let row = phoneme_core::Recording {
        id: id.clone(),
        started_at,
        duration_ms,
        audio_path: audio_path.to_string_lossy().into_owned(),
        in_place: false,
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        tags: vec![],
        speaker_names: vec![],
    };
    if let Err(e) = state.catalog.insert(&row).await {
        // Clean up the WAV we just wrote — no row means it's orphaned.
        let _ = tokio::fs::remove_file(&audio_path).await;
        return Response::Err(IpcError {
            kind: error_to_kind(&e),
            message: e.to_string(),
        });
    }

    let payload = HookPayload {
        id: id.clone(),
        timestamp: started_at,
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    if let Err(e) = state.inbox.enqueue(&payload).await {
        // No queue entry means this import would never be processed — roll the
        // catalog row and the canonical WAV back so it can't sit in the list
        // stuck on Transcribing forever. The caller can simply retry.
        let _ = state.catalog.delete(&id).await;
        let _ = tokio::fs::remove_file(&audio_path).await;
        return Response::Err(IpcError {
            kind: error_to_kind(&e),
            message: e.to_string(),
        });
    }

    state.events.emit(DaemonEvent::RecordingStopped {
        id: id.clone(),
        duration_ms,
        audio_path: audio_path.to_string_lossy().into_owned(),
        meeting_id: None,
    });
    tracing::info!(id = %id, source = %path, ms = duration_ms, "imported recording");
    Response::Ok(serde_json::json!({ "id": id.to_string() }))
}

fn error_to_kind(e: &phoneme_core::Error) -> IpcErrorKind {
    use phoneme_core::Error::*;
    match e {
        AlreadyRecording { .. } => IpcErrorKind::AlreadyRecording,
        NotRecording => IpcErrorKind::NotRecording,
        NotFound { .. } => IpcErrorKind::NotFound,
        InvalidConfig(_) => IpcErrorKind::InvalidConfig,
        WhisperUnreachable { .. } => IpcErrorKind::WhisperUnreachable,
        WhisperTimeout { .. } => IpcErrorKind::WhisperTimeout,
        HookFailed { .. } | HookTimeout { .. } => IpcErrorKind::HookFailed,
        DaemonNotRunning => IpcErrorKind::DaemonNotRunning,
        PipeInUse { .. } => IpcErrorKind::PipeInUse,
        ShuttingDown => IpcErrorKind::ShuttingDown,
        Io(_) => IpcErrorKind::Io,
        _ => IpcErrorKind::Internal,
    }
}

fn serialize_response<T: serde::Serialize>(val: T) -> Response {
    match serde_json::to_value(val) {
        Ok(v) => Response::Ok(v),
        Err(e) => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: format!("serialization failed: {e}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_size_cap_rejects_oversized_files() {
        // At or below the cap is accepted; one byte over is rejected.
        assert!(!exceeds_import_size_cap(0));
        assert!(!exceeds_import_size_cap(MAX_IMPORT_BYTES));
        assert!(exceeds_import_size_cap(MAX_IMPORT_BYTES + 1));
        // A clearly-oversized file (3 GiB) is rejected.
        assert!(exceeds_import_size_cap(3 * 1024 * 1024 * 1024));
    }

    #[test]
    fn hook_allowlist_accepts_only_configured_commands() {
        let configured = vec![
            "powershell -File C:\\hooks\\save.ps1".to_string(),
            "  notify-send {transcript}  ".to_string(), // padded in config
        ];
        // Exact configured command is allowed.
        assert!(hook_command_allowed(
            "powershell -File C:\\hooks\\save.ps1",
            &configured
        ));
        // Whitespace differences around the command don't matter (trimmed both sides).
        assert!(hook_command_allowed(
            "notify-send {transcript}",
            &configured
        ));
        // A command not in the list is rejected — this is the exec channel we close.
        assert!(!hook_command_allowed("calc.exe", &configured));
        assert!(!hook_command_allowed(
            "powershell -Command Remove-Item C:\\ -Recurse",
            &configured
        ));
        // Empty / whitespace-only requests are never allowed.
        assert!(!hook_command_allowed("", &configured));
        assert!(!hook_command_allowed("   ", &configured));
        // With no configured hooks, nothing is allowed.
        assert!(!hook_command_allowed("anything", &[]));
    }

    #[test]
    fn audio_path_guard_only_accepts_paths_under_audio_dir() {
        let dir = std::path::Path::new("/data/phoneme/audio");
        // A normal recording path under the audio dir is accepted.
        assert!(audio_path_is_ours(
            "/data/phoneme/audio/2026-06-08/rec.wav",
            dir
        ));
        // The audio dir itself is trivially "under" itself.
        assert!(audio_path_is_ours("/data/phoneme/audio", dir));
        // Paths outside the audio dir are rejected.
        assert!(!audio_path_is_ours("/etc/passwd", dir));
        // A sibling that merely shares a name prefix is rejected (component-wise
        // starts_with, not a string prefix).
        assert!(!audio_path_is_ours("/data/phoneme/audio-evil/x.wav", dir));
        // `..` traversal that would climb out is rejected even if it textually
        // begins under the audio dir.
        assert!(!audio_path_is_ours(
            "/data/phoneme/audio/../../etc/passwd",
            dir
        ));
    }

    // ── RetranscribeRecording model override (#49 regression) ──────────────

    use crate::app_state::AppState;
    use phoneme_core::config::{Config, TranscriptionBackend, WhisperMode};
    use phoneme_core::types::{Recording, RecordingStatus};
    use phoneme_core::RecordingId;

    async fn override_test_state(tmp: &std::path::Path, cfg: Config) -> AppState {
        std::env::set_var("PHONEME_DATA_LOCAL", tmp.join("data"));
        AppState::new(cfg).await.expect("build test AppState")
    }

    /// Insert a minimal Done recording row so a retranscribe has something to act
    /// on, and return its id.
    async fn insert_done_recording(state: &AppState) -> RecordingId {
        let id = RecordingId::new();
        let row = Recording {
            id: id.clone(),
            started_at: chrono::Local::now(),
            duration_ms: 1000,
            audio_path: "C:/phoneme/audio/x.wav".into(),
            transcript: Some("hello".into()),
            model: Some("ggml-base.en".into()),
            status: RecordingStatus::Done,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            tags: vec![],
            speaker_names: vec![],
        };
        state.catalog.insert(&row).await.unwrap();
        id
    }

    /// THE #49 REGRESSION: a model-override re-transcription for a Local
    /// (bundled) recording must NOT mutate the process-global whisper config.
    /// The old code wrote the override model into the shared config, which the
    /// whisper supervisor polls and restarts on — and which the queue worker's
    /// post-run reload reverted, restarting it again. That double restart raced
    /// every other queued/preview transcription (which read the same global
    /// config) and mass-failed them. The override must instead be recorded for
    /// just this one job in `pending_overrides`.
    #[tokio::test]
    async fn model_override_retranscribe_does_not_mutate_global_config() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = "C:/models/ggml-base.en.bin".into();
        cfg.whisper.bundled_server_port = 5809;
        let state = override_test_state(tmp.path(), cfg).await;

        let id = insert_done_recording(&state).await;
        // Snapshot the configured model BEFORE the request.
        let model_path_before = state.config.load().whisper.model_path.clone();
        let port_before = state.config.load().whisper.bundled_server_port;

        let resp = handle_request(
            Request::RetranscribeRecording {
                id: id.clone(),
                model: Some("C:/models/ggml-large-v3.bin".into()),
                run_hooks: None,
                post_process: None,
                all_overrides: None,
            },
            &state,
        )
        .await;
        assert!(
            matches!(resp, Response::Ok(_)),
            "retranscribe should be accepted"
        );

        // The GLOBAL config is untouched — this is the crux of the fix. The
        // supervisor never sees a model change here, so it never thrashes.
        let after = state.config.load();
        assert_eq!(
            after.whisper.model_path, model_path_before,
            "global whisper.model_path must NOT change on a model-override retranscribe"
        );
        assert_eq!(
            after.whisper.bundled_server_port, port_before,
            "global whisper port must be unchanged"
        );

        // The override is instead recorded against just this recording id, to be
        // applied by the pipeline when this single job runs. (Scoped so the std
        // MutexGuard drops before the await below — clippy::await_holding_lock.)
        {
            let pending = state.pending_overrides.lock().unwrap();
            assert_eq!(
                pending.get(&id).map(String::as_str),
                Some("C:/models/ggml-large-v3.bin"),
                "the per-job override should be queued for this recording only"
            );
        }

        // And the recording was put back into the transcribing state + enqueued.
        let rec = state.catalog.get(&id).await.unwrap().unwrap();
        assert_eq!(rec.status, RecordingStatus::Transcribing);
    }

    /// A retranscribe WITHOUT a model override must not create a phantom override
    /// entry (so a plain re-run always uses the configured model).
    #[tokio::test]
    async fn retranscribe_without_model_records_no_override() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = "C:/models/ggml-base.en.bin".into();
        let state = override_test_state(tmp.path(), cfg).await;

        let id = insert_done_recording(&state).await;
        let resp = handle_request(
            Request::RetranscribeRecording {
                id: id.clone(),
                model: None,
                run_hooks: Some(false),
                post_process: Some(false),
                all_overrides: None,
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok(_)));
        assert!(
            state.pending_overrides.lock().unwrap().get(&id).is_none(),
            "no model override should be recorded when none was requested"
        );
    }
}
