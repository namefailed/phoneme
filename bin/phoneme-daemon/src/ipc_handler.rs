//! IPC request routing — where every client request meets the daemon.
//!
//! [`ipc_server`](crate::ipc_server) hands each accepted pipe connection to
//! `handle_connection`, which loops:
//!   1. Read one Request (leniently — an unknown variant from a newer client
//!      is answered with an error, never a dropped connection).
//!   2. Call `handle_request` to produce a Response.
//!   3. Send the Response.
//!   4. Repeat until the client closes.
//!
//! `SubscribeEvents` is special — it permanently converts the connection
//! into a one-way `DaemonEvent` stream fed from the [`crate::event_bus`],
//! closing it when the subscriber lags.
//!
//! `handle_request` is the single dispatch point for the whole wire contract
//! (see `phoneme-ipc::schema` for the per-request documentation). Position
//! in the chain: most handlers read/write the catalog directly and return;
//! the ones that create transcription work (`RecordStop` via the recorder,
//! `ImportRecording`, `RetranscribeRecording`) enqueue into the inbox for
//! the queue worker. Handlers must stay fast — anything slow (hook re-fires,
//! LLM cleanup/summary re-runs, import decoding) runs in a spawned task or
//! on a blocking thread and reports through DaemonEvents, because one stalled
//! handler would stall every queued request on that connection (and the
//! tray's single-connection bridge with it).
//!
//! Security invariants enforced here: `RefireHook` only runs commands already
//! in the configured hook allowlist (S-C2); `HookTest` output is
//! secret-redacted on both outcomes; `DeleteRecording` only unlinks audio
//! under the configured audio dir; `ImportRecording` canonicalizes the path
//! and enforces a size cap before decoding.

use crate::app_state::AppState;
use phoneme_core::hook::redact_secrets;
use phoneme_core::{HookMetadata, HookPayload, HookRunner, RecordingStatus};
use phoneme_ipc::{
    DaemonEvent, IpcError, IpcErrorKind, NamedPipeConnection, PipelineStage, Request, Response,
    ServerRequest,
};

/// How long the `Shutdown` handler waits after returning its Ok response
/// before actually triggering the shutdown. The response write itself takes
/// microseconds — this just guarantees the reply is on the pipe before the
/// process begins to exit, so the caller always sees the acknowledgement.
const SHUTDOWN_REPLY_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// Minimum *calibrated* relevance (0..1) a semantic-only hit must clear to
/// surface. Hybrid search ranks by per-chunk best-match cosine fused (RRF)
/// with the FTS5 lexical ranking, so this is no longer a fragile raw-cosine
/// floor that silently dropped good paraphrase matches: a lexical (exact-term)
/// hit is never filtered by this, and the score is calibrated so 0.12 ≈
/// "barely related". See `catalog::hybrid_search`. `MoreLikeThis` applies the
/// same floor to its pure-vector ranking so both search paths agree on what's
/// too weak to show.
const SEMANTIC_MIN_RELEVANCE: f32 = 0.12;

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
        Request::DaemonStatus => {
            // Bundled whisper-server ports: `preferred` is the configured
            // value, `effective` is the port the supervisor actually bound —
            // it falls back to a free port when the preferred one is held by
            // another app, and is `null` while that server isn't running.
            // Clients probing the local server (the tray's connection test,
            // doctor wiring) should dial the effective port when present.
            let cfg = state.config.load();
            Response::Ok(serde_json::json!({
                "running": true,
                "pid": std::process::id(),
                "version": env!("CARGO_PKG_VERSION"),
                "whisper_preferred_port": cfg.whisper.bundled_server_port,
                "whisper_effective_port": state.whisper_ports.main(),
                "preview_whisper_preferred_port":
                    cfg.preview_whisper.as_ref().map(|p| p.bundled_server_port),
                "preview_whisper_effective_port": state.whisper_ports.preview(),
                "dictation_whisper_preferred_port":
                    cfg.in_place.stt.as_ref().map(|s| s.bundled_server_port),
                "dictation_whisper_effective_port": state.whisper_ports.dictation(),
            }))
        }
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
        Request::RecordStart {
            mode,
            in_place,
            recipe_id,
            whisper_model,
            source,
        } => {
            match state.recorder.start(state, mode, in_place, source).await {
                Ok(id) => {
                    // Custom-hotkey overrides: a binding that named a recipe / STT
                    // model stashes them against THIS recording's id, mirroring how
                    // `RetranscribeRecording` populates the per-job ledgers. The
                    // pipeline consumes (and removes) them in `run`. Empty/None =
                    // the normal record path (global default recipe + configured
                    // model), so non-custom recordings are untouched. See
                    // `stash_hotkey_overrides`.
                    stash_hotkey_overrides(state, &id, recipe_id, whisper_model);
                    Response::Ok(serde_json::json!({ "id": id.to_string() }))
                }
                Err(e) => err_response(&e),
            }
        }
        Request::StartMeeting => match state.recorder.start_meeting(state).await {
            Ok(meeting_id) => Response::Ok(serde_json::json!({ "meeting_id": meeting_id })),
            Err(e) => err_response(&e),
        },
        Request::StopMeeting => match state.recorder.stop_meeting(state).await {
            Ok(meeting_id) => Response::Ok(serde_json::json!({ "meeting_id": meeting_id })),
            Err(e) => err_response(&e),
        },
        Request::MeetingToggle => {
            // Atomic toggle: the recorder holds a guard across the read+act so a
            // double-tapped hotkey can't race two starts (or two stops). See
            // `DaemonRecorder::toggle_meeting`.
            match state.recorder.toggle_meeting(state).await {
                Ok(started) => Response::Ok(serde_json::json!({ "started": started })),
                Err(e) => err_response(&e),
            }
        }
        Request::RecordStop => match state.recorder.stop(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => err_response(&e),
        },
        Request::RecordToggle {
            in_place,
            recipe_id,
            whisper_model,
            source,
        } => {
            if state.recorder.current().await.is_some() {
                // Stop half of the toggle: there is no NEW recording to attach the
                // binding's overrides to (the active one was started with its own,
                // if any), so the recipe/model fields are intentionally ignored here.
                match state.recorder.stop(state).await {
                    Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
                    Err(e) => err_response(&e),
                }
            } else {
                match state
                    .recorder
                    .start(state, phoneme_core::RecordMode::Hold, in_place, source)
                    .await
                {
                    Ok(id) => {
                        // Start half: stash the binding's recipe/model overrides
                        // against the new recording id (see `RecordStart`).
                        stash_hotkey_overrides(state, &id, recipe_id, whisper_model);
                        Response::Ok(serde_json::json!({ "id": id.to_string() }))
                    }
                    Err(e) => err_response(&e),
                }
            }
        }
        Request::RecordPause => match state.recorder.pause(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => err_response(&e),
        },
        Request::RecordResume => match state.recorder.resume(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => err_response(&e),
        },
        Request::RecordCancel => match state.recorder.cancel(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => err_response(&e),
        },
        Request::ListRecordings { filter } => match state.catalog.list(&filter).await {
            Ok(rows) => serialize_response(rows),
            Err(e) => err_response(&e),
        },
        Request::GetRecording { id } => match state.catalog.get(&id).await {
            Ok(Some(r)) => serialize_response(r),
            Ok(None) => not_found(format!("recording {id} not found")),
            Err(e) => err_response(&e),
        },
        Request::ListAiActivity {
            recording_id,
            limit,
        } => match state
            .catalog
            .list_ai_activity(recording_id.as_deref(), limit as i64)
            .await
        {
            Ok(rows) => serialize_response(rows),
            Err(e) => err_response(&e),
        },
        Request::ListSavedSearches => match state.catalog.list_saved_searches().await {
            Ok(rows) => serialize_response(rows),
            Err(e) => err_response(&e),
        },
        Request::UpsertSavedSearch {
            id,
            name,
            filter_json,
        } => match state
            .catalog
            .upsert_saved_search(&id, &name, &filter_json)
            .await
        {
            Ok(()) => Response::Ok(serde_json::json!({})),
            Err(e) => err_response(&e),
        },
        Request::DeleteSavedSearch { id } => match state.catalog.delete_saved_search(&id).await {
            Ok(removed) => Response::Ok(serde_json::json!({ "removed": removed })),
            Err(e) => err_response(&e),
        },
        Request::ListMeeting { meeting_id } => {
            match state.catalog.list_by_meeting(&meeting_id).await {
                Ok(rows) => serialize_response(rows),
                Err(e) => err_response(&e),
            }
        }
        // An unknown id yields an empty list, not NotFound — "no segments" is
        // a normal state (pre-capture recordings, providers without timing)
        // and callers treat the two identically.
        Request::GetSegments { id } => match state.catalog.segments_for(&id).await {
            Ok(segments) => serialize_response(segments),
            Err(e) => err_response(&e),
        },
        // Like GetSegments, an unknown id yields an empty list (not NotFound):
        // "no words" is a normal state (pre-capture recordings, providers
        // without per-word timing). Each object carries an explicit 0-based
        // `idx` (the array order) so the frontend can rely on it without
        // re-deriving it from position — `TranscriptWord` itself stores no idx,
        // so we attach it here via enumerate.
        Request::GetWords { id } => match state.catalog.words_for(&id).await {
            Ok(words) => {
                let with_idx: Vec<serde_json::Value> = words
                    .into_iter()
                    .enumerate()
                    .map(|(idx, w)| {
                        serde_json::json!({
                            "idx": idx,
                            "start_ms": w.start_ms,
                            "end_ms": w.end_ms,
                            "text": w.text,
                            // Powers the Synced view's spacing (whisper's word-start
                            // marker); without it the view space-joins every token
                            // and shows "I don 't know" / "over ste pped".
                            "leading_space": w.leading_space,
                            "speaker": w.speaker,
                            "confidence": w.confidence,
                        })
                    })
                    .collect();
                serialize_response(with_idx)
            }
            Err(e) => err_response(&e),
        },
        Request::SemanticSearch { query, limit } => {
            // Clamp the client-supplied limit so a huge value can't force an
            // unbounded result allocation + JSON serialization over the pipe.
            let limit = limit.min(MAX_SEARCH_RESULTS);
            // Clone the Arc and drop the read guard before embedding: ONNX
            // inference is CPU-bound and runs under a std Mutex inside the
            // embedder, so doing it inline would block this Tokio worker (and
            // every config-reload writer) for the whole inference. Hand it to
            // spawn_blocking, matching the ingest path in pipeline.rs.
            let embedder = state.embedder.read().await.as_ref().cloned();
            if let Some(embedder) = embedder {
                let q = query.clone();
                let embed_res = tokio::task::spawn_blocking(move || embedder.embed_query(&q)).await;
                match embed_res {
                    Ok(Ok(query_vec)) => match state
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
                        Err(e) => err_response(&e),
                    },
                    Ok(Err(e)) => Response::Err(IpcError {
                        kind: IpcErrorKind::Internal,
                        message: format!("embedding failed: {e}"),
                    }),
                    Err(e) => Response::Err(IpcError {
                        kind: IpcErrorKind::Internal,
                        message: format!("embedding task failed: {e}"),
                    }),
                }
            } else {
                Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: "Semantic search is not enabled or model is missing.".to_string(),
                })
            }
        }
        Request::MoreLikeThis { id, limit } => {
            // Clamp the client-supplied limit (see SemanticSearch) so it can't
            // force an unbounded allocation.
            let limit = limit.min(MAX_SEARCH_RESULTS);
            // No embedder needed: the source recording's STORED vectors are the
            // query (that's the whole point — recall is free once indexed), so
            // this works even while the embedding model isn't loaded. The
            // catalog returns a clear "isn't indexed yet" error when the
            // recording has no vectors; forward it verbatim for the UI/CLI.
            match state
                .catalog
                .more_like_this(&id, limit, SEMANTIC_MIN_RELEVANCE)
                .await
            {
                Ok(results) => {
                    // Same `[{ recording, score }]` shape as SemanticSearch so
                    // clients reuse the relevance-chip rendering unchanged.
                    let mut full_results = Vec::new();
                    for (rec_id, score) in results {
                        if let Ok(Some(r)) = state.catalog.get(&rec_id).await {
                            full_results.push(serde_json::json!({
                                "recording": r,
                                "score": score,
                            }));
                        }
                    }
                    Response::Ok(serde_json::Value::Array(full_results))
                }
                Err(e) => err_response(&e),
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
                // Re-embed the WHOLE library with the current model, IN PLACE,
                // one recording at a time — never an upfront global wipe. The old
                // code did `clear_all_embeddings()` first, so a crash/kill/model-
                // unload between the clear and the end of the background loop left
                // the entire library permanently un-embedded with no recovery.
                //
                // `embed_and_store` → `upsert_chunk_embeddings` replaces a single
                // recording's chunks atomically (DELETE-then-INSERT in one tx), so
                // each step swaps that recording's old-model vectors for new-model
                // ones with no gap. If the pass is interrupted, the recordings not
                // yet reached keep their old-model embeddings and stay searchable;
                // the worst case is a partly-migrated library, not a wiped one.
                // Returns immediately; the work runs in the background.
                let bg = state.clone();
                tokio::spawn(async move {
                    if bg.embedder.read().await.is_none() {
                        return;
                    }
                    // Every recording with a transcript (no chunk-presence filter:
                    // we want to OVERWRITE existing vectors, not skip them).
                    let filter = phoneme_core::ListFilter::default();
                    match bg.catalog.list(&filter).await {
                        Ok(records) => {
                            let total = records.len();
                            tracing::info!(
                                "re-embedding {total} recordings in place with the current model"
                            );
                            let mut done = 0usize;
                            for r in records {
                                let Some(t) = r.transcript.as_ref().filter(|t| !t.is_empty())
                                else {
                                    continue;
                                };
                                // Re-acquire the embedder PER ITEM: this loop runs
                                // for minutes on a big library, and holding the
                                // read guard across it blocks every config-reload
                                // write. Clone the Arc, drop the guard, then embed
                                // — writers interleave between items. Gone mid-run
                                // (semantic search turned off) = stop; recordings
                                // already done keep their fresh vectors and the
                                // rest keep their old (still-searchable) ones.
                                let embedder = bg.embedder.read().await.as_ref().cloned();
                                let Some(embedder) = embedder else {
                                    tracing::info!(
                                        "re-embed stopped after {done}/{total}: embedding model unloaded"
                                    );
                                    return;
                                };
                                crate::pipeline::embed_and_store(embedder, &bg.catalog, &r.id, t)
                                    .await;
                                done += 1;
                            }
                            tracing::info!("re-embed complete ({done}/{total} recordings)");
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "re-embed: failed to list recordings")
                        }
                    }
                });
                ok_null()
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
                ok_null()
            }
            Ok(None) => not_found(format!("recording {id} not found")),
            Err(e) => err_response(&e),
        },
        Request::DeleteSession {
            meeting_id,
            keep_audio,
        } => match state.catalog.list_by_meeting(&meeting_id).await {
            Ok(tracks) if !tracks.is_empty() => {
                // Delete each track exactly like DeleteRecording: row first (an
                // error there leaves that track's audio untouched), then the WAV
                // unless keep_audio — and only when it's under our audio dir. One
                // track failing doesn't abandon the rest; each removed track emits
                // its own RecordingDeleted so every view drops it.
                let total = tracks.len();
                let mut deleted = 0usize;
                let mut last_err = None;
                for r in tracks {
                    if let Err(e) = state.catalog.delete(&r.id).await {
                        // Error, not warn: a track that survives the delete is a
                        // real partial failure the client must hear about (it's
                        // reflected in the error response below), not a routine
                        // best-effort miss like an already-gone audio file.
                        tracing::error!(id = %r.id, session = %meeting_id, error = %e, "session delete: track row delete failed");
                        last_err = Some(e);
                        continue;
                    }
                    if !keep_audio {
                        if audio_path_is_ours(&r.audio_path, &state.paths.audio_dir) {
                            if let Err(e) = tokio::fs::remove_file(&r.audio_path).await {
                                tracing::warn!(path = %r.audio_path, error = %e, "session delete: audio removal failed");
                            }
                        } else {
                            tracing::warn!(path = %r.audio_path, "refusing to delete audio file outside the audio directory");
                        }
                    }
                    state
                        .events
                        .emit(DaemonEvent::RecordingDeleted { id: r.id.clone() });
                    deleted += 1;
                }
                // Report any failure to the client instead of silently returning
                // Ok when some tracks were removed — matching DeleteRecording,
                // which always surfaces a delete error. The removed tracks already
                // emitted RecordingDeleted (so views drop them); a partial failure
                // returns an error carrying the deleted/total counts so the client
                // knows the session is only partly gone.
                match last_err {
                    None => ok_null(),
                    Some(e) if deleted == 0 => err_response(&e),
                    Some(e) => Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: format!(
                            "session partly deleted: {deleted}/{total} tracks removed, last error: {e}"
                        ),
                    }),
                }
            }
            Ok(_) => not_found(format!("meeting {meeting_id} not found")),
            Err(e) => err_response(&e),
        },
        Request::UpdateTranscript { id, text } => {
            match state.catalog.update_user_transcript(&id, &text).await {
                Ok(()) => {
                    // Re-flow the per-word / per-segment timing layers onto the
                    // edited text so the Synced + Timeline views (and click-to-seek)
                    // follow the edit. Best-effort: a failure here must not fail the
                    // save — the prose is already persisted. Gated by an opt-out so
                    // users who prefer the original machine timings can disable it.
                    if state.config.load().editor.resync_views_on_edit {
                        match state.catalog.words_for(&id).await {
                            Ok(old_words) => {
                                if let Some(r) =
                                    phoneme_core::realign::realign_transcript(&text, &old_words)
                                {
                                    if let Err(e) = state.catalog.replace_words(&id, &r.words).await
                                    {
                                        tracing::warn!(id = %id, error = %e, "re-align: failed to store re-flowed words");
                                    }
                                    if let Err(e) =
                                        state.catalog.replace_segments(&id, &r.segments).await
                                    {
                                        tracing::warn!(id = %id, error = %e, "re-align: failed to store re-flowed segments");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(id = %id, error = %e, "re-align: could not load words; leaving timing layers untouched");
                            }
                        }
                    }

                    let embedder = state.embedder.read().await.as_ref().cloned();
                    if let Some(embedder) = embedder {
                        crate::pipeline::embed_and_store(embedder, &state.catalog, &id, &text)
                            .await;
                    }

                    state.events.emit(DaemonEvent::TranscriptUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
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
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::GetOriginalTranscript { id } => {
            match state.catalog.get_original_transcript(&id).await {
                Ok(original) => serialize_response(original),
                Err(e) => err_response(&e),
            }
        }
        Request::GetCleanTranscript { id } => match state.catalog.get_clean_transcript(&id).await {
            Ok(clean) => serialize_response(clean),
            Err(e) => err_response(&e),
        },
        Request::SetFavorite { id, favorite } => {
            match state.catalog.set_favorite(&id, favorite).await {
                Ok(()) => ok_null(),
                Err(e) => err_response(&e),
            }
        }
        Request::SetRecordingTitle { id, title } => {
            // A blank title means "clear back to auto" — same as None. `Some`
            // marks the title user-owned, so the pipeline never overwrites it;
            // `None` resets ownership and the next run generates a fresh one.
            let title = title
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty());
            let is_auto = title.is_none();
            // A user/CLI title write carries no model — pass `None`, which also
            // clears any stale auto-title model so a user title never shows one.
            match state
                .catalog
                .set_title(&id, title.as_deref(), is_auto, None)
                .await
            {
                Ok(true) => {
                    // Same event a transcript edit emits — open views re-fetch
                    // the recording and pick the new title up.
                    state.events.emit(DaemonEvent::TranscriptUpdated { id });
                    ok_null()
                }
                Ok(false) => not_found(format!("no recording {}", id.as_str())),
                Err(e) => err_response(&e),
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
                        // Read the migrated `tags` Enrichment ENTRY (provider /
                        // model / prompt) so editing it in the Playbook changes
                        // what an on-demand re-run does — the Playbook is the
                        // source of truth. Fall back to the legacy `[auto_tag]`
                        // section when no such entry exists (a user deleted it).
                        match crate::pipeline::entry_config_for_target(&cfg, "tags") {
                            Some((llm_cfg, prompt)) => {
                                crate::pipeline::suggest_tags_with(
                                    state,
                                    &cfg,
                                    &id,
                                    &transcript,
                                    llm_cfg,
                                    &prompt,
                                )
                                .await;
                            }
                            None => {
                                crate::pipeline::suggest_tags(state, &cfg, &id, &transcript).await;
                            }
                        }
                        ok_null()
                    }
                }
                Ok(None) => not_found(format!("no recording {}", id.as_str())),
                Err(e) => err_response(&e),
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
                    Err(e) => err_response(&e),
                },
                Err(e) => err_response(&e),
            }
        }
        Request::ClearAllTagSuggestions => match state.catalog.clear_all_tag_suggestions().await {
            Ok(cleared) => {
                state
                    .events
                    .emit(DaemonEvent::AllTagSuggestionsCleared { cleared });
                Response::Ok(serde_json::json!({ "cleared": cleared }))
            }
            Err(e) => err_response(&e),
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
                        ok_null()
                    }
                    Err(e) => err_response(&e),
                }
            }
            Ok(None) => not_found(format!("no recording {}", id.as_str())),
            Err(e) => err_response(&e),
        },
        Request::UpdateNotes { id, notes } => match state.catalog.update_notes(&id, &notes).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::NotesUpdated { id });
                ok_null()
            }
            Err(e) => err_response(&e),
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
                    // Implicit enrollment (#9): naming a speaker folds its
                    // captured voiceprint into the cross-recording library;
                    // clearing the name un-enrolls it. Best-effort and a no-op
                    // when no voiceprint was captured (cloud-diarized recordings)
                    // — recognition is a convenience, never a reason to fail the
                    // rename.
                    let enrolled_id = if name.trim().is_empty() {
                        if let Err(e) = state
                            .catalog
                            .unenroll_speaker(id.as_str(), speaker_label)
                            .await
                        {
                            tracing::warn!(id = %id.as_str(), label = speaker_label, "voiceprint unenroll failed: {e}");
                        }
                        None
                    } else {
                        match state
                            .catalog
                            .enroll_speaker(id.as_str(), speaker_label, &name)
                            .await
                        {
                            Ok(nid) => nid,
                            Err(e) => {
                                tracing::warn!(id = %id.as_str(), label = speaker_label, "voiceprint enroll failed: {e}");
                                None
                            }
                        }
                    };
                    state
                        .events
                        .emit(DaemonEvent::SpeakerNameUpdated { id: id.clone() });

                    // Name propagation (V5): when the speaker actually enrolled
                    // into the library, optionally back-fill that name onto the
                    // SAME unnamed voice in other recordings, per policy. Naming
                    // never fails over propagation — it's a convenience layered on
                    // top, so any error here is logged and swallowed.
                    let cfg = state.config.load();
                    let propagation = match enrolled_id {
                        Some(nid) if cfg.diarization.recognize_speakers => {
                            speaker_name_propagation(state, &nid, &cfg.diarization).await
                        }
                        // No enrollment (cleared name / cloud-diarized / recognition
                        // off) → nothing to propagate.
                        _ => serde_json::json!({ "policy": "off", "applied": 0, "candidates": [] }),
                    };
                    Response::Ok(serde_json::json!({ "propagation": propagation }))
                }
                Err(e) => err_response(&e),
            }
        }
        // ── In-recording speaker correction (U1) ─────────────────────────
        // Each op keeps `transcript_segments` authoritative and rebuilds the
        // prose `[Speaker N]:` markers in one transaction (catalog side), so
        // every view the user sees agrees. They emit `SpeakerNameUpdated` so
        // open clients refresh the recording (segments, names, prose).
        Request::ReassignSegmentSpeaker { id, idx, new_label } => {
            match state.catalog.reassign_segment(&id, idx, new_label).await {
                Ok(()) => {
                    state
                        .events
                        .emit(DaemonEvent::SpeakerNameUpdated { id: id.clone() });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::MergeSpeakers {
            id,
            from_label,
            into_label,
        } => match state
            .catalog
            .merge_speakers(&id, from_label, into_label)
            .await
        {
            Ok(()) => {
                state
                    .events
                    .emit(DaemonEvent::SpeakerNameUpdated { id: id.clone() });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::SplitSpeaker {
            id,
            label,
            segment_idxs,
            new_label,
        } => match state
            .catalog
            .split_speaker(&id, label, &segment_idxs, new_label)
            .await
        {
            Ok(()) => {
                state
                    .events
                    .emit(DaemonEvent::SpeakerNameUpdated { id: id.clone() });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::RecognizeSpeakers { id } => {
            let cfg = state.config.load();
            if cfg.diarization.recognize_speakers {
                // V2 score normalization: when off (default), use the raw cosine
                // bar exactly as before; when on, switch to the z-score bar.
                let (mode, threshold) = voiceprint_scorer(&cfg.diarization);
                match state
                    .catalog
                    .recognize_speakers_for(id.as_str(), threshold, mode)
                    .await
                {
                    Ok(suggestions) => serialize_response(suggestions),
                    Err(e) => err_response(&e),
                }
            } else {
                serialize_response(Vec::<phoneme_core::types::SpeakerSuggestion>::new())
            }
        }
        Request::DismissSpeakerSuggestion { id, speaker_label } => {
            match state
                .catalog
                .dismiss_speaker_suggestion(id.as_str(), speaker_label)
                .await
            {
                Ok(()) => ok_null(),
                Err(e) => err_response(&e),
            }
        }
        Request::ListNamedVoices => match state.catalog.list_named_voices().await {
            Ok(voices) => serialize_response(voices),
            Err(e) => err_response(&e),
        },
        Request::RenameNamedVoice { id, name } => {
            match state.catalog.rename_named_voice(&id, &name).await {
                Ok(()) => ok_null(),
                Err(e) => err_response(&e),
            }
        }
        Request::MergeNamedVoices { from_id, into_id } => {
            match state.catalog.merge_named_voices(&from_id, &into_id).await {
                Ok(merged) => Response::Ok(serde_json::json!({ "merged": merged })),
                Err(e) => err_response(&e),
            }
        }
        Request::ForgetNamedVoice { id } => match state.catalog.forget_named_voice(&id).await {
            Ok(removed) => Response::Ok(serde_json::json!({ "removed": removed })),
            Err(e) => err_response(&e),
        },
        Request::UndoForgetNamedVoice { id } => match state.catalog.undo_forget(&id).await {
            Ok(restored) => Response::Ok(serde_json::json!({ "restored": restored })),
            Err(e) => err_response(&e),
        },
        Request::ImportRecording { path } => import_recording(state, path).await,
        Request::ReimportFromDisk { dry_run } => reimport_from_disk(state, dry_run).await,
        Request::RebuildCatalog => {
            // Refuse while capture is live — clearing the table would orphan a
            // row still being written. The user must stop recording first.
            if state.recorder.is_busy().await {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::AlreadyRecording,
                    message: "can't rebuild the catalog while recording — stop the recording first"
                        .into(),
                });
            }
            // Clear every row (cascade takes tags/segments/words/embeddings), then
            // re-import from disk: with the table empty, every WAV is an orphan, so
            // the existing reimport re-links them all as Queued. WAVs are never
            // touched, so this is recoverable to the audio even though transcripts
            // and tags are lost (and re-derived by re-transcription).
            match state.catalog.clear_all_recordings().await {
                Ok(removed) => {
                    tracing::info!(
                        removed,
                        "catalog rebuild: cleared all rows; re-importing from disk"
                    );
                    reimport_from_disk(state, false).await
                }
                Err(e) => err_response(&e),
            }
        }
        Request::RetranscribeRecording {
            id,
            model,
            run_hooks,
            post_process,
            all_overrides,
            recipe_id,
        } => match state.catalog.get(&id).await {
            Ok(Some(r)) => {
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
                        state
                            .pending_overrides
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                    } else {
                        state
                            .pending_overrides
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(id.clone(), m.to_string());
                    }
                }
                // One-time recipe override (Re-run → "Run with recipe"): record the
                // chosen Playbook recipe against this id; `pipeline::run` claims it
                // from `pending_recipe` and resolves that chain instead of the
                // global `default`. Empty clears any stale request, like the model.
                if let Some(rid) = recipe_id {
                    let rid = rid.trim();
                    let mut map = state
                        .pending_recipe
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if rid.is_empty() {
                        map.remove(&id);
                    } else {
                        map.insert(id.clone(), rid.to_string());
                    }
                }
                // The one-time LLM/hook overrides (hooks toggle, post-processing
                // opt-out, and the Re-run → "All" cleanup/summary/title values)
                // are ALSO recorded per-recording — NEVER written into the
                // process-global config. A temp-global write here raced a
                // concurrent ReloadConfig (it could be clobbered, or leak its
                // forced-on pipeline onto another queued job). `pipeline::run`
                // applies these to THIS job's config clone only. Mirrors the
                // per-recording model override above.
                let rerun = crate::app_state::PendingRerun {
                    run_hooks,
                    post_process,
                    all_overrides,
                };
                {
                    let mut map = state
                        .pending_all_overrides
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if rerun.is_empty() {
                        // Clear any stale request so a prior re-run's overrides
                        // can't leak onto this plain retranscribe.
                        map.remove(&id);
                    } else {
                        map.insert(id.clone(), rerun);
                    }
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
                        // Queued, not Transcribing: it waits behind the serial
                        // inbox; the pipeline flips it to Transcribing when the
                        // worker claims it.
                        if let Err(e) = state
                            .catalog
                            .update_status(&id, RecordingStatus::Queued)
                            .await
                        {
                            tracing::error!("failed to update status to queued: {e}");
                        }
                        ok_null()
                    }
                    Err(e) => {
                        // Enqueue failed: this job never reaches `pipeline::run`
                        // (the sole place these per-recording ledgers are
                        // otherwise claimed), so the entries we just stashed
                        // would leak keyed by this id. Drop them on this terminal
                        // path — honoring the "removed on every terminal path"
                        // invariant — recovering from a poisoned lock like the
                        // other pending_* sites do. `pending_focused_app` isn't
                        // populated for a retranscribe, but a defensive remove
                        // keeps the contract airtight.
                        state
                            .pending_overrides
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        state
                            .pending_all_overrides
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        state
                            .pending_recipe
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        state
                            .pending_focused_app
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        err_response(&e)
                    }
                }
            }
            Ok(None) => not_found(format!("recording {id} not found")),
            Err(e) => err_response(&e),
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
                // Post-cutover, the "configured hooks" are the `default` recipe's
                // Hook-step commands (where migrate_hooks moved [hook].commands),
                // each with the Phoneme path tokens expanded — same allowlist
                // semantics as before. Webhook-only Hook steps have no command and
                // are skipped (RefireHook only re-runs commands, as it always has).
                let configured: Vec<String> = {
                    use phoneme_core::config::{expand_cmd, PlaybookKind};
                    let mut cmds = Vec::new();
                    if let Some(recipe) = cfg.recipes.iter().find(|r| r.id == "default") {
                        for step_id in &recipe.steps {
                            if let Some(e) = cfg.playbook.iter().find(|e| &e.id == step_id) {
                                let c = e.hook.command.trim();
                                if e.kind == PlaybookKind::Hook && !c.is_empty() {
                                    cmds.push(expand_cmd(c));
                                }
                            }
                        }
                    }
                    cmds
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
                ok_null()
            }
            Ok(Some(_)) => Response::Err(IpcError {
                kind: IpcErrorKind::Internal,
                message: "no transcript to fire hook against".into(),
            }),
            Ok(None) => not_found(format!("recording {id} not found")),
            Err(e) => err_response(&e),
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
            // Thread the bundled servers' LIVE ports into the backend probes so
            // a startup port fallback can't make Doctor probe the dead
            // configured port. The supervisors publish these in `whisper_ports`
            // the same way the pipeline reads them via `apply`.
            let ports = phoneme_core::doctor::EffectiveWhisperPorts {
                main: state.whisper_ports.main(),
                preview: state.whisper_ports.preview(),
                in_place: state.whisper_ports.dictation(),
            };
            let mut checks = phoneme_core::doctor::run_local_checks(&cfg);
            checks.extend(phoneme_core::doctor::run_backend_checks_with_ports(&cfg, &ports).await);
            // Daemon-side: needs the catalog + an audio-dir scan, so it can't live
            // in phoneme-core's stateless checks.
            checks.push(orphan_audio_check(state).await);
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
                Ok(()) => ok_null(),
                Err(e) => err_response(&e),
            }
        }
        Request::SkipCurrentStage => {
            // Wakes whichever LLM stage is currently streaming (no-op when none
            // is — the notify has no waiter then and stores nothing).
            state.skip_stage.notify_waiters();
            tracing::info!("skip-current-stage requested via IPC");
            ok_null()
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
                (Err(e), _) | (_, Err(e)) => err_response(&e),
            }
        }
        Request::ReorderQueue { ids } => match state.inbox.set_order(&ids).await {
            Ok(()) => {
                crate::queue_worker::emit_queue_depth(state).await;
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::CancelQueued { id } => match state.inbox.cancel_pending(&id).await {
            Ok(true) => {
                // Mark the recording Cancelled — terminal, so it isn't stuck
                // showing "transcribing", but distinct from the failed states:
                // the user chose this, nothing broke. Re-runnable later.
                let _ = state
                    .catalog
                    .update_status(&id, RecordingStatus::Cancelled)
                    .await;
                state
                    .events
                    .emit(DaemonEvent::RecordingCancelled { id: id.clone() });
                crate::queue_worker::emit_queue_depth(state).await;
                ok_null()
            }
            Ok(false) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: "recording is not in the pending queue (already processing or finished)"
                    .into(),
            }),
            Err(e) => err_response(&e),
        },
        Request::SetQueuePaused { paused } => match state.inbox.set_paused(paused).await {
            Ok(()) => {
                // Nudge the panel so the pause state reflects immediately.
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "paused": paused }))
            }
            Err(e) => err_response(&e),
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
            Err(e) => err_response(&e),
        },
        Request::ClearFailed => match state.inbox.clear_failed().await {
            Ok(removed) => {
                // Refresh the depth so the panel's failed badge clears at once.
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "removed": removed }))
            }
            Err(e) => err_response(&e),
        },
        Request::DismissFailed { id } => match state.inbox.dismiss_failed(&id).await {
            Ok(removed) => {
                // Refresh the depth so the failed badge reflects the new count.
                if removed {
                    crate::queue_worker::emit_queue_depth(state).await;
                }
                Response::Ok(serde_json::json!({ "removed": removed }))
            }
            Err(e) => err_response(&e),
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
                ok_null()
            } else {
                Response::Err(IpcError {
                    kind: IpcErrorKind::NotFound,
                    message: "recording is not the item currently being processed".into(),
                })
            }
        }
        Request::CancelAllQueued => match state.inbox.cancel_all_pending().await {
            Ok(ids) => {
                // Mark each recording Cancelled (terminal, but not a failure)
                // so it isn't stuck showing "transcribing", mirroring
                // single-item CancelQueued.
                for id in &ids {
                    let _ = state
                        .catalog
                        .update_status(id, RecordingStatus::Cancelled)
                        .await;
                    state
                        .events
                        .emit(DaemonEvent::RecordingCancelled { id: id.clone() });
                }
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "removed": ids.len() }))
            }
            Err(e) => err_response(&e),
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
            // The test command is caller-supplied and its output is shown in
            // the UI/CLI verbatim — a script that echoes its environment or a
            // config file would hand any key it prints to the renderer. Mask
            // credential-shaped values on BOTH outcomes before the text
            // crosses the pipe: the Ok path carries stderr directly, and the
            // HookFailed error embeds the stderr tail in its message.
            match runner.run(&sample).await {
                Ok(result) => Response::Ok(serde_json::json!({
                    "exit_code": result.exit_code,
                    "duration_ms": result.duration_ms,
                    "stderr_tail": redact_secrets(&result.stderr_tail),
                })),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: redact_secrets(&e.to_string()),
                }),
            }
        }
        Request::Shutdown => {
            tracing::info!("shutdown requested via IPC");
            // Reply first, exit second: the trigger is delayed so the Ok
            // response (written by `handle_connection` the moment this arm
            // returns) reaches the pipe before the process starts tearing
            // down — the caller (`phoneme daemon stop`, the tray's Quit) must
            // never be left waiting on a reply that died with the daemon.
            let coordinator = state.shutdown.clone();
            tokio::spawn(async move {
                tokio::time::sleep(SHUTDOWN_REPLY_GRACE).await;
                // Trigger the shared coordinator `main` waits on: it stops
                // the recorder, the workers, and every Owned child, then exits.
                coordinator.trigger();
            });
            ok_null()
        }
        Request::ListTags => match state.catalog.list_tags().await {
            Ok(tags) => Response::Ok(serde_json::to_value(tags).unwrap_or_default()),
            Err(e) => err_response(&e),
        },
        Request::ListAllTags => match state.catalog.list_all_tags().await {
            Ok(tags) => Response::Ok(serde_json::to_value(tags).unwrap_or_default()),
            Err(e) => err_response(&e),
        },
        Request::AddTag { name, color } => {
            match state.catalog.add_tag(&name, color.as_deref()).await {
                Ok(tag) => {
                    state.events.emit(DaemonEvent::TagCreated { id: tag.id });
                    Response::Ok(serde_json::to_value(tag).unwrap_or_default())
                }
                Err(e) => err_response(&e),
            }
        }
        Request::UpdateTag { id, name, color } => {
            match state.catalog.update_tag(id, &name, color.as_deref()).await {
                Ok(tag) => {
                    state.events.emit(DaemonEvent::TagUpdated { id });
                    Response::Ok(serde_json::to_value(tag).unwrap_or_default())
                }
                Err(e) => err_response(&e),
            }
        }
        Request::DeleteTag { id } => match state.catalog.delete_tag(id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagDeleted { id });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::AttachTag {
            recording_id,
            tag_id,
        } => match state.catalog.attach_tag(&recording_id, tag_id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagAttached { tag_id });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::DetachTag {
            recording_id,
            tag_id,
        } => match state.catalog.detach_tag(&recording_id, tag_id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagDetached { tag_id });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::TagsFor { recording_id } => match state.catalog.tags_for(&recording_id).await {
            Ok(tags) => Response::Ok(serde_json::to_value(tags).unwrap_or_default()),
            Err(e) => err_response(&e),
        },
        Request::TagUsageCounts => match state.catalog.tag_usage_counts().await {
            Ok(counts) => Response::Ok(serde_json::to_value(counts).unwrap_or_default()),
            Err(e) => err_response(&e),
        },
        Request::KindCounts => match state.catalog.kind_counts().await {
            Ok(counts) => Response::Ok(serde_json::to_value(counts).unwrap_or_default()),
            Err(e) => err_response(&e),
        },
        Request::MergeTags { from_id, into_id } => {
            match state.catalog.merge_tags(from_id, into_id).await {
                Ok(()) => {
                    // The source tag is gone; consumers refresh on TagDeleted.
                    state.events.emit(DaemonEvent::TagDeleted { id: from_id });
                    ok_null()
                }
                Err(e) => err_response(&e),
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

                    // Drop the cached local diarization pipeline when
                    // `[diarization]` changed (backend switch / model path) —
                    // the next run reloads under the new config, and switching
                    // away from Local frees the model RAM immediately.
                    state
                        .transcription
                        .diarizer_cache()
                        .invalidate_if_stale(&cfg_arc.diarization);

                    drop(cfg_arc);
                    drop(embedder_guard);

                    // Start/stop idle pre-roll pre-capture to match the new
                    // config (e.g. user just toggled pre_roll_ms).
                    state.recorder.sync_preroll(state).await;
                    ok_null()
                }
                Err(e) => Response::Err(IpcError {
                    kind: IpcErrorKind::InvalidConfig,
                    message: format!("failed to load config: {e}"),
                }),
            }
        }
        Request::SubscribeEvents => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "subscribe_events is handled by the streaming path in handle_connection"
                .into(),
        }),
    }
}

/// Hard cap on how many results a single `SemanticSearch` / `MoreLikeThis` request
/// may return. The client picks the limit, so this bounds the result Vec + the
/// JSON serialized back over the pipe — a huge `limit` can't force an unbounded
/// allocation. Far above any real UI/CLI page size.
const MAX_SEARCH_RESULTS: usize = 1000;

/// Stash a custom-hotkey recording's per-job overrides against its freshly minted
/// recording id, so `pipeline::run` resolves the binding's recipe + transcribes
/// with its model. Two ledgers, both already proven by `RetranscribeRecording`:
///
///  • `whisper_model` → `pending_overrides` (the existing per-job model override
///    map): the pipeline applies it via `apply_model_override` for one job, then
///    restores — the same #49-safe path a model-override retranscribe uses.
///  • `recipe_id` → `pending_recipe` (the parallel recipe ledger): the pipeline
///    passes it to `resolve_recipe`, falling back to the `default` recipe when the
///    id is empty or names a deleted recipe.
///
/// Both are written ONLY when non-empty, so a normal (non-custom-hotkey) record —
/// which sends `None` — leaves the recording on the global default recipe +
/// configured model. The maps are ephemeral: a daemon restart between stash and
/// the pipeline claim drops the override and the job runs the default recipe +
/// configured model (the documented `pending_overrides` contract). A leftover
/// entry can't leak onto another recording (each `RecordingId` is unique), and the
/// entries are claimed-and-removed on EVERY terminal path: `pipeline::run` removes
/// both EARLY — alongside the model/all-overrides removals, before transcription —
/// so a permanently-failed recording leaves nothing, and `DaemonRecorder::cancel`
/// removes both in its single-recording arm so a recording canceled mid-capture
/// (which never reaches `pipeline::run`) leaves nothing either.
fn stash_hotkey_overrides(
    state: &AppState,
    id: &phoneme_core::RecordingId,
    recipe_id: Option<String>,
    whisper_model: Option<String>,
) {
    if let Some(model) = whisper_model {
        let model = model.trim();
        if !model.is_empty() {
            state
                .pending_overrides
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.clone(), model.to_string());
        }
    }
    if let Some(recipe) = recipe_id {
        let recipe = recipe.trim();
        if !recipe.is_empty() {
            state
                .pending_recipe
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.clone(), recipe.to_string());
        }
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

/// Doctor check for ORPHANED AUDIO: `.wav` files on disk that have no catalog
/// row. They accumulate when recordings are deleted with "keep the audio file",
/// and a `--reimport` would resurrect them — so surface the count rather than
/// let it grow silently and surprise the user later. Reuses the re-import scan
/// + `all_ids`, so it counts exactly what "Re-import from disk" would re-link.
async fn orphan_audio_check(state: &AppState) -> phoneme_core::doctor::CheckResult {
    let existing: std::collections::HashSet<phoneme_core::RecordingId> = state
        .catalog
        .all_ids()
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let audio_dir = state.paths.audio_dir.clone();
    let count = tokio::task::spawn_blocking(move || scan_audio_dir(&audio_dir))
        .await
        .map(|cands| {
            cands
                .into_iter()
                .filter(|c| !existing.contains(&c.id))
                .count()
        })
        .unwrap_or(0);
    phoneme_core::doctor::orphan_audio_check_result(count)
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
            return not_found(format!("recording {id} not found"));
        }
        Err(e) => {
            return err_response(&e);
        }
    };

    // Cleanup operates on text — there must be something to clean.
    if recording.transcript.is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "no transcript to run cleanup on".into(),
        });
    }

    // Resolve the BASE (llm_cfg, prompt) from the migrated `cleanup` Playbook
    // ENTRY so editing it in the Playbook changes what an on-demand Re-run
    // Cleanup does — the Playbook is the source of truth, exactly like the
    // summary/tags re-runs read their migrated entries. `cleanup_entry_config`
    // falls back to the legacy `[llm_post_process]` config + prompt when the
    // entry is gone (a user deleted it), so behavior is never worse than today.
    // The Re-run modal's one-time overrides then layer ON TOP and still win;
    // none of this is persisted (the config is local to the spawned task).
    let CleanupOverrides {
        model,
        provider,
        prompt,
        api_url,
        api_key,
    } = overrides;
    let (base_llm, base_prompt) = crate::pipeline::cleanup_entry_config(&state.config.load());
    // Layer the one-shot model + prompt overrides via the SHARED helper that
    // `rerun_summary` (and the tests) use, so the layering rule lives in exactly
    // one place. A non-empty override wins; a blank/whitespace one is ignored —
    // a blank prompt would strip the cleanup instructions.
    let (mut llm_cfg, resolved_prompt) = crate::pipeline::apply_oneshot_overrides(
        base_llm,
        base_prompt,
        model.as_deref(),
        prompt.as_deref(),
    );
    llm_cfg.prompt = resolved_prompt;
    // Provider / endpoint / key overrides are cleanup-only (the summary re-run
    // has no such fields) so they apply directly around the shared base. Note
    // `cleanup_entry_config` already forced the step enabled — the GUI disables
    // the Re-run Cleanup option when cleanup is off, and the provider check
    // below still blocks a `none`/blank provider.
    if let Some(p) = provider {
        let p = p.trim();
        if !p.is_empty() {
            llm_cfg.provider = p.to_string();
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
    // Audit trail: a one-time override can point this run's cleanup at a different
    // provider/endpoint. Log the resolved target (never the API key) so a
    // redirect is visible in the logs rather than silent.
    tracing::info!(
        id = %id,
        provider = %llm_cfg.provider,
        api_url = %llm_cfg.api_url,
        model = %llm_cfg.model,
        "re-run cleanup resolved (one-time overrides applied; API key never logged)"
    );

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
        // defensively rather than unwrapped. Going through the run-resolver
        // here (not at validation above) keeps the Ollama auto-launch off the
        // IPC connection too.
        let Some(provider) = crate::pipeline::llm_provider_for_run(&task_state, &llm_cfg).await
        else {
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
                // by a text-only re-clean, so preserve whatever was stored —
                // both the flag and the diarizer model).
                if let Err(e) = task_state
                    .catalog
                    .update_processing_meta(
                        &id,
                        Some(&llm_cfg.model),
                        recording.diarized,
                        recording.diarization_model.as_deref(),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "rerun_cleanup: failed to update processing meta");
                }

                // A re-run was requested precisely because the prior cleanup
                // failed — so clear the terminal CleanupFailed status now that it
                // succeeded, otherwise the recording reads as failed forever even
                // though it cleaned fine. `update_transcript` above already cleared
                // the error_kind/error_message columns; only the status remained.
                // Best-effort + scoped to CleanupFailed so a re-run never masks an
                // unrelated terminal status (e.g. HookFailed).
                if recording.status == RecordingStatus::CleanupFailed {
                    if let Err(e) = task_state
                        .catalog
                        .update_status(&id, RecordingStatus::Done)
                        .await
                    {
                        tracing::warn!(error = %e, "rerun_cleanup: failed to clear CleanupFailed status");
                    }
                }

                // Re-embed the new text so semantic search stays consistent,
                // mirroring the pipeline and UpdateTranscript paths.
                let embedder = task_state.embedder.read().await.as_ref().cloned();
                if let Some(embedder) = embedder {
                    crate::pipeline::embed_and_store(embedder, &task_state.catalog, &id, &cleaned)
                        .await;
                }

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

    ok_null()
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
            return not_found(format!("recording {id} not found"));
        }
        Err(e) => {
            return err_response(&e);
        }
    };

    let transcript = recording.transcript.clone().unwrap_or_default();
    if transcript.trim().is_empty() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "no transcript to summarize".into(),
        });
    }

    // Resolve the BASE (llm_cfg, prompt) from the migrated `summary` Playbook
    // ENTRY so editing it in the Playbook changes what an on-demand re-run does
    // — the Playbook is the source of truth. The Re-run modal's one-time
    // overrides (a non-empty model / prompt) then layer ON TOP and still win.
    // When no `summary` entry exists (a user deleted it) fall back to the legacy
    // [summary]/[llm_post_process] path (`generate_summary`) so behavior is never
    // worse than today. `Resolution` carries whichever path we took to the probe
    // and the spawned task.
    let cfg = (**state.config.load()).clone();
    let model = model.filter(|m| !m.trim().is_empty());
    let prompt = prompt.filter(|p| !p.trim().is_empty());

    enum Resolution {
        /// The migrated `summary` entry drives this run; one-shot overrides
        /// already layered on. `generate_summary_with` dispatches it directly.
        Entry {
            llm_cfg: phoneme_core::config::LlmPostProcessConfig,
            prompt: String,
            endpoint_hint: Option<String>,
        },
        /// No `summary` entry — the legacy `[summary]` section drives this run
        /// via `generate_summary`, with the one-shot overrides baked into `cfg`.
        /// Boxed: `Config` is large and this is the rare path (clippy
        /// large_enum_variant — keep the common `Entry` arm cheap to move).
        Legacy {
            cfg: Box<phoneme_core::config::Config>,
        },
    }

    let resolution = match crate::pipeline::entry_config_for_target(&cfg, "summary") {
        Some((base_llm, base_prompt)) => {
            // Layer the one-shot model + prompt overrides via the SHARED helper
            // that `rerun_cleanup` (and the tests) use — the single source of
            // truth for "non-empty override wins, blank is ignored".
            let (llm_cfg, entry_prompt) = crate::pipeline::apply_oneshot_overrides(
                base_llm,
                base_prompt,
                model.as_deref(),
                prompt.as_deref(),
            );
            // Name the endpoint in any real-error message (a stale per-step URL
            // is the classic cause and invisible in a generic message).
            let endpoint_hint = {
                let url = llm_cfg.api_url.trim();
                (!url.is_empty()).then(|| url.to_string())
            };
            Resolution::Entry {
                llm_cfg,
                prompt: entry_prompt,
                endpoint_hint,
            }
        }
        None => {
            // Bake the one-shot overrides into the [summary] section of a clone,
            // then let `generate_summary` resolve it exactly as it did before.
            let mut cfg_legacy = cfg.clone();
            if let Some(m) = &model {
                cfg_legacy.summary.model = m.clone();
            }
            if let Some(p) = &prompt {
                cfg_legacy.summary.prompt = p.clone();
            }
            Resolution::Legacy {
                cfg: Box::new(cfg_legacy),
            }
        }
    };

    // Require a usable LLM provider up front so the user gets a clear error
    // rather than a silent no-op. The summary generators re-check defensively.
    let probe = match &resolution {
        Resolution::Entry { llm_cfg, .. } => llm_cfg.clone(),
        Resolution::Legacy { cfg } => crate::pipeline::summary_llm_config(cfg),
    };
    if state.llm.provider(&probe).is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::InvalidConfig,
            message: "no LLM provider configured for summaries (set a summary or [llm_post_process] provider)"
                .into(),
        });
    }

    // Snapshot the status so the spawned task can clear a stale SummarizeFailed
    // on success without re-fetching (RecordingStatus is Copy).
    let prev_status = recording.status;
    let task_state = state.clone();
    tokio::spawn(async move {
        // Surface this re-run in the queue as an active "Summarizing…" item.
        task_state.events.emit(DaemonEvent::PipelineStageChanged {
            id: id.clone(),
            stage: PipelineStage::Summarizing,
        });
        let result = match resolution {
            Resolution::Entry {
                llm_cfg,
                prompt,
                endpoint_hint,
            } => {
                crate::pipeline::generate_summary_with(
                    &task_state,
                    &id,
                    &transcript,
                    llm_cfg,
                    &prompt,
                    endpoint_hint.as_deref(),
                )
                .await
            }
            Resolution::Legacy { cfg } => {
                crate::pipeline::generate_summary(&task_state, &cfg, &id, &transcript).await
            }
        };
        match result {
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
                // Clear a stale SummarizeFailed status now that the summary
                // succeeded — otherwise the recording reads as failed forever
                // even though the re-run worked. Best-effort + scoped to
                // SummarizeFailed so a re-run never masks an unrelated terminal
                // status. (The error_kind/error_message columns are left as-is;
                // the list/detail "failed" state keys off `status`, which is now
                // Done, so the recording no longer surfaces as failed.)
                if prev_status == RecordingStatus::SummarizeFailed {
                    if let Err(e) = task_state
                        .catalog
                        .update_status(&id, RecordingStatus::Done)
                        .await
                    {
                        tracing::warn!(error = %e, "rerun_summary: failed to clear SummarizeFailed status");
                    }
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

    ok_null()
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
            return not_found(format!("could not resolve path {path}: {e}"));
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
        // Queued, not Transcribing: the import rides the serial inbox; the
        // pipeline flips it to Transcribing when the worker claims it.
        status: RecordingStatus::Queued,
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
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        tags: vec![],
        speaker_names: vec![],
    };
    if let Err(e) = state.catalog.insert(&row).await {
        // Clean up the WAV we just wrote — no row means it's orphaned.
        let _ = tokio::fs::remove_file(&audio_path).await;
        return err_response(&e);
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
        // stuck on Queued forever. The caller can simply retry.
        let _ = state.catalog.delete(&id).await;
        let _ = tokio::fs::remove_file(&audio_path).await;
        return err_response(&e);
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

/// A `.wav` on disk whose RecordingId has no catalog row — a candidate to
/// re-link in [`reimport_from_disk`].
struct ReimportCandidate {
    id: phoneme_core::RecordingId,
    path: std::path::PathBuf,
    duration_ms: i64,
    started_at: chrono::DateTime<chrono::Local>,
}

/// Reconstruct a RecordingId from a day folder (`YYYY-MM-DD`) + a file stem
/// (`HHmmssNNN`) — the inverse of the `audio_dir/<day>/<stem>.wav` layout that
/// `RecordingId::day_folder()`/`file_stem()` produce. `None` for anything that
/// isn't a valid id (e.g. a user-dropped file with a different name).
fn id_from_path_parts(day_name: &str, stem: &str) -> Option<phoneme_core::RecordingId> {
    let date_digits: String = day_name.chars().filter(|c| *c != '-').collect();
    phoneme_core::RecordingId::parse(format!("{date_digits}T{stem}"))
}

/// The original wall-clock time encoded in a RecordingId (`YYYYMMDDTHHmmssNNN`),
/// so a re-imported row keeps its real timestamp instead of "now". Falls back to
/// the current time only if the slices somehow don't parse (the id is already
/// shape-validated by `parse`).
fn started_at_from_id(id: &phoneme_core::RecordingId) -> chrono::DateTime<chrono::Local> {
    use chrono::{Local, NaiveDate, NaiveTime, TimeZone};
    let s = id.as_str();
    let build = || -> Option<chrono::DateTime<Local>> {
        let y: i32 = s.get(0..4)?.parse().ok()?;
        let mo: u32 = s.get(4..6)?.parse().ok()?;
        let d: u32 = s.get(6..8)?.parse().ok()?;
        let h: u32 = s.get(9..11)?.parse().ok()?;
        let mi: u32 = s.get(11..13)?.parse().ok()?;
        let se: u32 = s.get(13..15)?.parse().ok()?;
        let dt = NaiveDate::from_ymd_opt(y, mo, d)?.and_time(NaiveTime::from_hms_opt(h, mi, se)?);
        Local.from_local_datetime(&dt).single()
    };
    build().unwrap_or_else(Local::now)
}

/// Synchronously walk `audio_dir/<YYYY-MM-DD>/<HHmmssNNN>.wav`, collecting every
/// `.wav` whose path reconstructs to a valid RecordingId. Blocking std::fs (run
/// off the runtime by the caller); no new crate dependency. Unreadable dirs are
/// skipped rather than failing the whole scan.
fn scan_audio_dir(audio_dir: &std::path::Path) -> Vec<ReimportCandidate> {
    let mut out = Vec::new();
    let Ok(days) = std::fs::read_dir(audio_dir) else {
        return out;
    };
    for day in days.flatten() {
        let day_path = day.path();
        if !day_path.is_dir() {
            continue;
        }
        let Some(day_name) = day_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(files) = std::fs::read_dir(&day_path) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("wav") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(id) = id_from_path_parts(day_name, stem) else {
                continue;
            };
            let duration_ms = phoneme_audio::wav::duration_ms(&p).unwrap_or(0);
            let started_at = started_at_from_id(&id);
            out.push(ReimportCandidate {
                id,
                path: p,
                duration_ms,
                started_at,
            });
        }
    }
    out
}

/// Re-link audio files that have no catalog row — the SAFE counterpart to the
/// destructive `doctor --rebuild-catalog`. Scans the audio dir, and for every
/// `.wav` whose RecordingId isn't already in the catalog inserts a `Queued` row
/// pointing at the EXISTING file (no copy, original id + timestamp preserved)
/// and enqueues it for the normal pipeline. Never deletes or mutates existing
/// rows. `dry_run` returns the count + paths without writing anything.
async fn reimport_from_disk(state: &AppState, dry_run: bool) -> Response {
    let existing: std::collections::HashSet<phoneme_core::RecordingId> =
        match state.catalog.all_ids().await {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => return err_response(&e),
        };

    let audio_dir = state.paths.audio_dir.clone();
    let candidates = match tokio::task::spawn_blocking(move || scan_audio_dir(&audio_dir)).await {
        Ok(c) => c,
        Err(e) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::Internal,
                message: format!("re-import scan task panicked: {e}"),
            });
        }
    };

    let orphans: Vec<ReimportCandidate> = candidates
        .into_iter()
        .filter(|c| !existing.contains(&c.id))
        .collect();

    if dry_run {
        let paths: Vec<String> = orphans
            .iter()
            .map(|c| c.path.to_string_lossy().into_owned())
            .collect();
        return Response::Ok(serde_json::json!({ "count": orphans.len(), "paths": paths }));
    }

    let mut count = 0usize;
    for c in orphans {
        let audio_path = c.path.to_string_lossy().into_owned();
        let row = phoneme_core::Recording {
            id: c.id.clone(),
            started_at: c.started_at,
            duration_ms: c.duration_ms,
            audio_path: audio_path.clone(),
            in_place: false,
            transcript: None,
            model: None,
            // Queued (not Transcribing): it rides the serial inbox; the worker
            // flips it to Transcribing when it claims the job — same as import.
            status: RecordingStatus::Queued,
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
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
        };
        if let Err(e) = state.catalog.insert(&row).await {
            tracing::warn!(id = %c.id, "re-import: failed to insert row, skipping: {e}");
            continue;
        }
        let payload = HookPayload {
            id: c.id.clone(),
            timestamp: c.started_at,
            transcript: String::new(),
            audio_path: audio_path.clone(),
            duration_ms: c.duration_ms,
            model: String::new(),
            metadata: HookMetadata::current(),
        };
        if let Err(e) = state.inbox.enqueue(&payload).await {
            // No queue entry means it'd sit stuck on Queued forever — roll the
            // row back (the file is untouched, so a later re-import retries it).
            let _ = state.catalog.delete(&c.id).await;
            tracing::warn!(id = %c.id, "re-import: failed to enqueue, rolled back: {e}");
            continue;
        }
        state.events.emit(DaemonEvent::RecordingStopped {
            id: c.id.clone(),
            duration_ms: c.duration_ms,
            audio_path,
            meeting_id: None,
        });
        count += 1;
    }
    tracing::info!(count, "re-imported orphaned recordings from disk");
    Response::Ok(serde_json::json!({ "count": count }))
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

/// The error arm shared by nearly every handler: a core error answered as
/// `Response::Err` with the standard kind mapping and the error's own text.
/// Wire-identical to spelling the `IpcError` out at the call site.
/// The (mode, threshold) pair the voiceprint scorer should use for a diarization
/// config — V2 score-norm aware. With norm `off` (default) it's the raw cosine
/// bar; with `s_norm`/`as_norm` it's the z-score bar. Shared by recognition and
/// V5 propagation so both judge "is this the same voice" the same way.
fn voiceprint_scorer(
    diar: &phoneme_core::config::DiarizationConfig,
) -> (phoneme_core::voiceprint::ScoreNorm, f32) {
    let mode = phoneme_core::voiceprint::ScoreNorm::from(diar.voiceprint_score_norm);
    let threshold = if mode == phoneme_core::voiceprint::ScoreNorm::Off {
        diar.voiceprint_match_threshold as f32
    } else {
        diar.voiceprint_score_norm_threshold as f32
    };
    (mode, threshold)
}

/// Run V5 name propagation for a just-enrolled named voice, returning the JSON the
/// `SetSpeakerName` response carries. Routes on `diar.name_propagation`:
/// `off` → no-op; `auto` → back-fill every candidate and report the count; `ask`
/// → return the candidate list for the UI to confirm (apply nothing). Best-effort
/// — any catalog error is logged and reported as an empty result, never failing
/// the rename.
async fn speaker_name_propagation(
    state: &AppState,
    named_voice_id: &str,
    diar: &phoneme_core::config::DiarizationConfig,
) -> serde_json::Value {
    use phoneme_core::config::NamePropagation;
    if diar.name_propagation == NamePropagation::Off {
        return serde_json::json!({ "policy": "off", "applied": 0, "candidates": [] });
    }
    let (mode, threshold) = voiceprint_scorer(diar);
    let candidates = match state
        .catalog
        .propagation_candidates(named_voice_id, threshold, mode)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(voice = %named_voice_id, "propagation candidate scan failed: {e}");
            return serde_json::json!({ "policy": "ask", "applied": 0, "candidates": [] });
        }
    };
    match diar.name_propagation {
        NamePropagation::Off => unreachable!("handled above"),
        NamePropagation::Auto => {
            let targets: Vec<(phoneme_core::id::RecordingId, i64)> = candidates
                .iter()
                .map(|c| (c.recording_id.clone(), c.speaker_label))
                .collect();
            let applied = match state
                .catalog
                .apply_propagation(named_voice_id, &targets)
                .await
            {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!(voice = %named_voice_id, "propagation apply failed: {e}");
                    0
                }
            };
            if applied > 0 {
                // The back-filled recordings now show the new name — nudge clients
                // to refresh those rows.
                for c in &candidates {
                    state.events.emit(DaemonEvent::SpeakerNameUpdated {
                        id: c.recording_id.clone(),
                    });
                }
            }
            serde_json::json!({ "policy": "auto", "applied": applied, "candidates": [] })
        }
        NamePropagation::Ask => {
            // Surface the candidates for the UI to confirm; change nothing now.
            serde_json::json!({
                "policy": "ask",
                "applied": 0,
                "candidates": serde_json::to_value(&candidates).unwrap_or(serde_json::Value::Array(vec![])),
            })
        }
    }
}

fn err_response(e: &phoneme_core::Error) -> Response {
    Response::Err(IpcError {
        kind: error_to_kind(e),
        message: e.to_string(),
    })
}

/// A `NotFound` error response. Callers format the message (the wording
/// varies per request and is part of the wire contract); this pins the kind.
fn not_found(message: String) -> Response {
    Response::Err(IpcError {
        kind: IpcErrorKind::NotFound,
        message,
    })
}

/// The bare `Ok(null)` acknowledgement most mutating requests answer with.
fn ok_null() -> Response {
    Response::Ok(serde_json::Value::Null)
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
    fn reimport_id_from_path_parts_round_trips() {
        // The real audio_dir layout: day folder + 9-char time stem -> 18-char id.
        let id = id_from_path_parts("2026-06-15", "014341016").unwrap();
        assert_eq!(id.as_str(), "20260615T014341016");
        assert_eq!(id.day_folder(), "2026-06-15");
        assert_eq!(id.file_stem(), "014341016");
        // A user-dropped file with a non-id name is skipped, not mis-relinked.
        assert!(id_from_path_parts("2026-06-15", "my-notes").is_none());
        assert!(id_from_path_parts("not-a-day", "014341016").is_none());
    }

    #[test]
    fn reimport_started_at_decodes_the_id_timestamp() {
        use chrono::{Datelike, Timelike};
        let id = phoneme_core::RecordingId::parse("20260615T014341016").unwrap();
        let dt = started_at_from_id(&id);
        assert_eq!((dt.year(), dt.month(), dt.day()), (2026, 6, 15));
        assert_eq!((dt.hour(), dt.minute(), dt.second()), (1, 43, 41));
    }

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
        // Explicit data-local (no global `set_var`) so parallel tests don't race —
        // see `AppState::new_in`.
        AppState::new_in(cfg, Some(tmp.join("data")))
            .await
            .expect("build test AppState")
    }

    /// `daemon_status` surfaces the bundled-server ports: the configured
    /// (preferred) one and the one the supervisor actually bound, so clients
    /// probing the local server dial it even after a port fallback.
    #[tokio::test]
    async fn daemon_status_reports_preferred_and_effective_ports() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.bundled_server_port = 5809;
        let state = override_test_state(tmp.path(), cfg).await;

        // Server not (yet) running: preferred mirrors config, effective null.
        let Response::Ok(v) = handle_request(Request::DaemonStatus, &state).await else {
            panic!("daemon_status should answer ok");
        };
        assert_eq!(v["whisper_preferred_port"], 5809);
        assert!(v["whisper_effective_port"].is_null());
        assert!(v["preview_whisper_preferred_port"].is_null());
        assert!(v["preview_whisper_effective_port"].is_null());

        // The supervisor published a fallback port: effective reports it
        // while preferred keeps naming the configured value.
        state.whisper_ports.set_main(Some(51234));
        let Response::Ok(v) = handle_request(Request::DaemonStatus, &state).await else {
            panic!("daemon_status should answer ok");
        };
        assert_eq!(v["whisper_preferred_port"], 5809);
        assert_eq!(v["whisper_effective_port"], 51234);
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
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
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
                recipe_id: None,
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

        // And the recording was put back into the queue (Queued; the worker
        // flips it to Transcribing when it claims the item) + enqueued.
        let rec = state.catalog.get(&id).await.unwrap().unwrap();
        assert_eq!(rec.status, RecordingStatus::Queued);
    }

    /// The Shutdown handler must REPLY before the daemon exits: the Ok is
    /// produced immediately while the coordinator trigger lags by the grace
    /// delay, so the caller (`phoneme daemon stop`, the tray's Quit) always
    /// reads its acknowledgement off the pipe before teardown begins.
    #[tokio::test]
    async fn shutdown_replies_before_triggering_the_coordinator() {
        let tmp = tempfile::tempdir().unwrap();
        let state = override_test_state(tmp.path(), Config::default()).await;

        let resp = handle_request(Request::Shutdown, &state).await;
        assert!(matches!(resp, Response::Ok(_)), "shutdown must ACK");
        assert!(
            !state.shutdown.signal.is_shutting_down(),
            "the trigger must lag the reply (grace delay), not race it"
        );

        // ...and the trigger must actually arrive shortly after the grace.
        let mut signal = state.shutdown.signal.clone();
        tokio::time::timeout(std::time::Duration::from_secs(5), signal.wait())
            .await
            .expect("shutdown must trigger after the grace delay");
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
                recipe_id: None,
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

    /// Cancelling a queued item must mark the recording `Cancelled` — NOT
    /// `TranscribeFailed`. A user removing their own item from the queue is not
    /// a failure: the old status lit the failed badge and listed the recording
    /// in the failure panel for something the user did on purpose.
    #[tokio::test]
    async fn cancel_queued_marks_recording_cancelled_not_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = override_test_state(tmp.path(), Config::default()).await;

        // A recording waiting in the queue: catalog row at Queued plus a
        // pending inbox payload (what RecordStop / import leave behind).
        let id = insert_done_recording(&state).await;
        state
            .catalog
            .update_status(&id, RecordingStatus::Queued)
            .await
            .unwrap();
        let payload = phoneme_core::HookPayload {
            id: id.clone(),
            timestamp: chrono::Local::now(),
            transcript: String::new(),
            audio_path: "C:/phoneme/audio/x.wav".into(),
            duration_ms: 1000,
            model: String::new(),
            metadata: HookMetadata::current(),
        };
        state.inbox.enqueue(&payload).await.unwrap();

        let resp = handle_request(Request::CancelQueued { id: id.clone() }, &state).await;
        assert!(matches!(resp, Response::Ok(_)), "cancel should succeed");

        let rec = state.catalog.get(&id).await.unwrap().unwrap();
        assert_eq!(
            rec.status,
            RecordingStatus::Cancelled,
            "a user cancel is Cancelled, never a failed status"
        );
    }

    /// CancelAllQueued ("clear queue") marks every removed item `Cancelled`,
    /// mirroring the single-item path.
    #[tokio::test]
    async fn cancel_all_queued_marks_recordings_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        let state = override_test_state(tmp.path(), Config::default()).await;

        let mut ids = Vec::new();
        for _ in 0..2 {
            let id = insert_done_recording(&state).await;
            state
                .catalog
                .update_status(&id, RecordingStatus::Queued)
                .await
                .unwrap();
            let payload = phoneme_core::HookPayload {
                id: id.clone(),
                timestamp: chrono::Local::now(),
                transcript: String::new(),
                audio_path: "C:/phoneme/audio/x.wav".into(),
                duration_ms: 1000,
                model: String::new(),
                metadata: HookMetadata::current(),
            };
            state.inbox.enqueue(&payload).await.unwrap();
            ids.push(id);
        }

        let resp = handle_request(Request::CancelAllQueued, &state).await;
        let Response::Ok(v) = resp else {
            panic!("cancel-all should succeed");
        };
        assert_eq!(v["removed"], 2);
        for id in &ids {
            let rec = state.catalog.get(id).await.unwrap().unwrap();
            assert_eq!(rec.status, RecordingStatus::Cancelled);
        }
    }

    /// HookTest output crosses the pipe to the tray/CLI, and the test command
    /// is caller-supplied — a script that dumps its environment must not hand
    /// credentials to the renderer. Both outcomes are redacted: the Ok path's
    /// `stderr_tail` and the `HookFailed` message (which embeds stderr).
    #[tokio::test]
    async fn hook_test_redacts_secrets_on_both_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = "C:/models/ggml-base.en.bin".into();
        let state = override_test_state(tmp.path(), cfg).await;

        // Ok path: the command succeeds but echoes a credential to stderr.
        #[cfg(windows)]
        let ok_cmd = "cmd /c \"echo password=hunter2secret 1>&2\"";
        #[cfg(not(windows))]
        let ok_cmd = "sh -c \"echo password=hunter2secret 1>&2\"";
        let resp = handle_request(
            Request::HookTest {
                custom_command: Some(ok_cmd.to_string()),
            },
            &state,
        )
        .await;
        match resp {
            Response::Ok(v) => {
                let tail = v["stderr_tail"].as_str().unwrap_or_default();
                assert!(
                    !tail.contains("hunter2secret"),
                    "secret leaked through HookTest stderr: {tail}"
                );
                assert!(
                    tail.contains("password=<redacted>"),
                    "mask expected in stderr_tail, got: {tail}"
                );
            }
            other => panic!("expected Ok, got {other:?}"),
        }

        // Err path: a failing command's stderr rides inside the HookFailed
        // message — the same redaction must apply there.
        #[cfg(windows)]
        let fail_cmd = "cmd /c \"echo token=topsecret123 1>&2 & exit 3\"";
        #[cfg(not(windows))]
        let fail_cmd = "sh -c \"echo token=topsecret123 1>&2; exit 3\"";
        let resp = handle_request(
            Request::HookTest {
                custom_command: Some(fail_cmd.to_string()),
            },
            &state,
        )
        .await;
        match resp {
            Response::Err(e) => {
                assert!(
                    !e.message.contains("topsecret123"),
                    "secret leaked through the HookTest failure message: {}",
                    e.message
                );
                assert!(
                    e.message.contains("token=<redacted>"),
                    "mask expected in the failure message, got: {}",
                    e.message
                );
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }
}
