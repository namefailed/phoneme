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
use phoneme_ipc::{DaemonEvent, IpcError, IpcErrorKind, NamedPipeConnection, Request, Response};

pub async fn handle_connection(mut conn: NamedPipeConnection, state: AppState) {
    loop {
        match conn.recv().await {
            Ok(Some(Request::SubscribeEvents)) => {
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
            Ok(Some(req)) => {
                let response = handle_request(req, &state).await;
                if let Err(e) = conn.send_response(response).await {
                    tracing::warn!(error = %e, "send_response failed");
                    return;
                }
            }
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "recv failed");
                return;
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
            Response::Ok(serde_json::json!({
                "recording": active.is_some(),
                "id": active.as_ref().map(|a| a.id.to_string()),
                "meeting": meeting_active,
            }))
        }
        Request::RecordStart { mode } => match state.recorder.start(state, mode.into()).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::StartMeeting => match state.recorder.start_meeting(state).await {
            Ok(session_id) => Response::Ok(serde_json::json!({ "session_id": session_id })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::StopMeeting => match state.recorder.stop_meeting(state).await {
            Ok(session_id) => Response::Ok(serde_json::json!({ "session_id": session_id })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::MeetingToggle => {
            let result = if state.recorder.meeting_active().await {
                state.recorder.stop_meeting(state).await
            } else {
                state.recorder.start_meeting(state).await
            };
            match result {
                Ok(session_id) => Response::Ok(serde_json::json!({ "session_id": session_id })),
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
        Request::RecordToggle => {
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
                    .start(state, phoneme_core::RecordMode::Oneshot.into())
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
            Ok(rows) => Response::Ok(serde_json::to_value(rows).unwrap_or(serde_json::Value::Null)),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::GetRecording { id } => match state.catalog.get(&id).await {
            Ok(Some(r)) => Response::Ok(serde_json::to_value(r).unwrap_or(serde_json::Value::Null)),
            Ok(None) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: format!("recording {id} not found"),
            }),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::ListSession { session_id } => {
            match state.catalog.list_by_session(&session_id).await {
                Ok(rows) => {
                    Response::Ok(serde_json::to_value(rows).unwrap_or(serde_json::Value::Null))
                }
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
        Request::SemanticSearch { query, limit } => {
            if let Some(embedder) = state.embedder.as_ref() {
                match embedder.embed(&query) {
                    Ok(query_vec) => match state.catalog.semantic_search(&query_vec, limit).await {
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
                    // Best-effort — the file may already be gone. Log, don't fail.
                    if let Err(e) = tokio::fs::remove_file(&r.audio_path).await {
                        tracing::warn!(
                            path = %r.audio_path,
                            error = %e,
                            "audio file removal failed"
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
                    state.events.emit(DaemonEvent::TranscriptUpdated { id });
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
                Ok(original) => {
                    Response::Ok(serde_json::to_value(original).unwrap_or(serde_json::Value::Null))
                }
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: e.to_string(),
                }),
            }
        }
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
        Request::ImportRecording { path } => import_recording(state, path).await,
        Request::RetranscribeRecording { id, model } => match state.catalog.get(&id).await {
            Ok(Some(r)) => {
                if let Some(m) = model {
                    let mut cfg = state.config.load().as_ref().clone();
                    cfg.whisper.model_path = m;
                    state.config.store(std::sync::Arc::new(cfg));
                    // Wait a moment for the supervisor to restart the server
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                let payload = HookPayload {
                    id: r.id.clone(),
                    timestamp: r.started_at,
                    transcript: String::new(),
                    audio_path: r.audio_path.clone(),
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
        Request::RefireHook { id } => match state.catalog.get(&id).await {
            Ok(Some(r)) if r.transcript.is_some() => {
                let payload = HookPayload {
                    id: r.id.clone(),
                    timestamp: r.started_at,
                    transcript: r.transcript.clone().unwrap_or_default(),
                    audio_path: r.audio_path.clone(),
                    duration_ms: r.duration_ms,
                    model: r.model.clone().unwrap_or_default(),
                    metadata: HookMetadata::current(),
                };
                // Load config once and extract what we need before spawning
                // the async task — avoids holding the Arc guard across await
                // points and eliminates the previous triple load.
                let cfg = state.config.load();
                let timeout = std::time::Duration::from_secs(cfg.hook.timeout_secs);
                // Expand path tokens (%APPDATA%, ~/) exactly as the queue
                // pipeline does, so a refired hook resolves identically.
                let commands = match cfg.expanded() {
                    Ok(c) => c.hook.commands,
                    Err(_) => cfg.hook.commands.clone(),
                };
                drop(cfg);
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
        Request::ReloadConfig => {
            tracing::info!("reloading config via IPC");
            match crate::load_config() {
                Ok(cfg) => {
                    state.config.store(std::sync::Arc::new(cfg));
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

/// Import an existing audio file: decode it to a canonical WAV under the audio
/// dir, insert a catalog row, and enqueue it for the normal transcription
/// pipeline. Mirrors `DaemonRecorder::stop` (catalog row at `Transcribing` +
/// `inbox.enqueue`) so an imported file is processed exactly like a mic
/// recording — the only difference is where the WAV came from.
async fn import_recording(state: &AppState, path: String) -> Response {
    let requested = std::path::PathBuf::from(&path);

    // Validate before doing any work, so the client gets a clean error.
    if !requested.exists() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::NotFound,
            message: format!("file not found: {path}"),
        });
    }

    // Canonicalize so the path we open is a fully-resolved, real filesystem
    // location (resolves `..`, symlinks, and relative components). The dialog
    // hands us absolute paths already; this hardens the arbitrary-client-path
    // bypass by ensuring we never act on a half-resolved or traversal path.
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
        session_id: None,
        track: None,
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
        return Response::Err(IpcError {
            kind: error_to_kind(&e),
            message: e.to_string(),
        });
    }

    state.events.emit(DaemonEvent::RecordingStopped {
        id: id.clone(),
        duration_ms,
        audio_path: audio_path.to_string_lossy().into_owned(),
        session_id: None,
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
}
