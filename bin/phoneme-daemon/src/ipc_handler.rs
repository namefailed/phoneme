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
        })),
        Request::RecordStatus => {
            let active = state.recorder.current().await;
            Response::Ok(serde_json::json!({
                "recording": active.is_some(),
                "id": active.as_ref().map(|a| a.id.to_string()),
            }))
        }
        Request::RecordStart { mode } => match state.recorder.start(state, mode.into()).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
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
            match state
                .catalog
                .update_transcript(&id, &text, "user-edit")
                .await
            {
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
        Request::ReplayRecording { id } => match state.catalog.get(&id).await {
            Ok(Some(r)) => {
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
                let runner = HookRunner::new(
                    state
                        .config
                        .load()
                        .hook
                        .commands
                        .first()
                        .cloned()
                        .unwrap_or_default(),
                    std::time::Duration::from_secs(state.config.load().hook.timeout_secs),
                );
                // Run the hook OFF the IPC connection. A hook can take up to
                // its full timeout (30s default); running it inline froze the
                // connection — and with it the single-connection Tauri bridge,
                // stalling every other UI request. The outcome is reported via
                // DaemonEvents, exactly like the queue pipeline.
                //
                // We deliberately do NOT re-enqueue (as ReplayRecording does):
                // the queue pipeline always re-transcribes first, which would
                // overwrite a user's manual transcript edit. RefireHook must
                // re-run only the hook against the stored transcript.
                let task_state = state.clone();
                let command = state
                    .config
                    .load()
                    .hook
                    .commands
                    .first()
                    .cloned()
                    .unwrap_or_default();
                tokio::spawn(async move {
                    let hook_id = payload.id.clone();
                    task_state.events.emit(DaemonEvent::HookStarted {
                        id: hook_id.clone(),
                    });
                    match runner.run(&payload).await {
                        Ok(result) => {
                            if let Err(e) = task_state
                                .catalog
                                .update_hook_result(
                                    &hook_id,
                                    &command,
                                    result.exit_code,
                                    result.duration_ms,
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
                                exit_code: result.exit_code,
                            });
                        }
                        Err(e) => {
                            if let Err(err) = task_state
                                .catalog
                                .update_status(&hook_id, RecordingStatus::HookFailed)
                                .await
                            {
                                tracing::error!("failed to update status to hook_failed: {err}");
                            }
                            task_state.events.emit(DaemonEvent::HookFailed {
                                id: hook_id,
                                error: e.to_string(),
                            });
                        }
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
            Ok(()) => Response::Ok(serde_json::Value::Null),
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        },
        Request::DetachTag {
            recording_id,
            tag_id,
        } => match state.catalog.detach_tag(&recording_id, tag_id).await {
            Ok(()) => Response::Ok(serde_json::Value::Null),
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
