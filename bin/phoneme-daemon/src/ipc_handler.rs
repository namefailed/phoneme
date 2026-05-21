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
            Ok(Some(req)) => {
                let response = handle_request(req, &state).await;
                if let Err(e) = conn.send_response(response).await {
                    tracing::warn!(error = %e, "send_response failed; closing connection");
                    break;
                }
            }
            Ok(None) => {
                tracing::debug!("client disconnected");
                break;
            }
            Err(e) => {
                tracing::warn!(error = %e, "recv failed; closing connection");
                break;
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
                let _ = state.catalog.delete(&id).await;
                if !keep_audio {
                    let _ = tokio::fs::remove_file(&r.audio_path).await;
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
            match state.catalog.update_transcript(&id, &text, "user-edit").await {
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
                        let _ = state
                            .catalog
                            .update_status(&id, RecordingStatus::Transcribing)
                            .await;
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
                    state.config.hook.command.clone(),
                    std::time::Duration::from_secs(state.config.hook.timeout_secs),
                );
                match runner.run(&payload).await {
                    Ok(result) => {
                        let _ = state
                            .catalog
                            .update_hook_result(
                                &id,
                                &state.config.hook.command,
                                result.exit_code,
                                result.duration_ms,
                            )
                            .await;
                        let _ = state
                            .catalog
                            .update_status(&id, RecordingStatus::Done)
                            .await;
                        state.events.emit(DaemonEvent::HookDone {
                            id,
                            exit_code: result.exit_code,
                        });
                        Response::Ok(serde_json::Value::Null)
                    }
                    Err(e) => {
                        let _ = state
                            .catalog
                            .update_status(&id, RecordingStatus::HookFailed)
                            .await;
                        state.events.emit(DaemonEvent::HookFailed {
                            id,
                            error: e.to_string(),
                        });
                        Response::Err(IpcError {
                            kind: error_to_kind(&e),
                            message: e.to_string(),
                        })
                    }
                }
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
        Request::HookTest => {
            let runner = HookRunner::new(
                state.config.hook.command.clone(),
                std::time::Duration::from_secs(state.config.hook.timeout_secs),
            );
            let sample = HookPayload {
                id: phoneme_core::RecordingId::new(),
                timestamp: chrono::Local::now(),
                transcript: "This is a test transcript for the hook.".into(),
                audio_path: String::new(),
                duration_ms: 0,
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
            Response::Ok(serde_json::Value::Null)
            // Actual shutdown coordination wired in Task 11.
        }
        Request::ReloadConfig => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "reload_config not implemented in v1".into(),
        }),
        Request::SubscribeEvents => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "subscribe_events is handled by the streaming path in handle_connection (Task 10)".into(),
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
        LlmUnreachable { .. } => IpcErrorKind::LlmUnreachable,
        LlmTimeout { .. } => IpcErrorKind::LlmTimeout,
        HookFailed { .. } | HookTimeout { .. } => IpcErrorKind::HookFailed,
        DaemonNotRunning => IpcErrorKind::DaemonNotRunning,
        PipeInUse { .. } => IpcErrorKind::PipeInUse,
        ShuttingDown => IpcErrorKind::ShuttingDown,
        Io(_) => IpcErrorKind::Io,
        _ => IpcErrorKind::Internal,
    }
}
