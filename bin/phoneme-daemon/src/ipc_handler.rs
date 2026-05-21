//! IPC request routing.
//!
//! Each accepted pipe connection runs `handle_connection`, which loops:
//!   1. Read one Request.
//!   2. Call `handle_request` to produce a Response.
//!   3. Send the Response.
//!   4. Repeat until the client closes.
//!
//! `SubscribeEvents` is special — it hijacks the connection for the rest of
//! its life and streams DaemonEvents.

use crate::app_state::AppState;
use phoneme_ipc::{IpcError, IpcErrorKind, NamedPipeConnection, Request, Response};

pub async fn handle_connection(mut conn: NamedPipeConnection, _state: AppState) {
    loop {
        match conn.recv().await {
            Ok(Some(req)) => {
                let response = handle_request(req).await;
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

pub async fn handle_request(req: Request) -> Response {
    match req {
        Request::DaemonStatus => Response::Ok(serde_json::json!({
            "running": true,
            "pid": std::process::id(),
            "stub": true,
        })),
        Request::RecordStatus => Response::Ok(serde_json::json!({
            "recording": false,
        })),
        _ => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "not yet implemented".into(),
        }),
    }
}
