//! REST error type and its mapping to HTTP status codes.
//!
//! A request that reaches a handler can fail in two distinct ways, and the two
//! map to different status families:
//!
//! - **Transport** — the daemon could not be reached, the pipe closed
//!   mid-request, or a frame was malformed. The daemon is the dependency a
//!   local bridge sits in front of, so an unreachable daemon is a `503 Service
//!   Unavailable` (try again once the daemon is up), not a client error.
//! - **Daemon error** — the request reached the daemon and it answered
//!   [`Response::Err`](phoneme_ipc::Response). Its [`IpcErrorKind`] carries the
//!   category, which we fold onto the closest HTTP status (`not_found` → 404,
//!   the bad-input kinds → 400, `shutting_down` → 503, everything else → 500).
//!
//! Plus one purely local case — a malformed `:id` path segment — that never
//! reaches the daemon and is a flat `400 Bad Request`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::Json;
use phoneme_ipc::{IpcError, IpcErrorKind, IpcTransportError};
use serde_json::json;

/// Everything a handler can fail with, before it becomes an HTTP response.
#[derive(Debug)]
pub enum RestError {
    /// The daemon could not be reached / the connection dropped (→ 503).
    Transport(IpcTransportError),
    /// The daemon answered with a structured error (status derived from
    /// [`IpcErrorKind`]).
    Daemon(IpcError),
    /// A path/query parameter was malformed before any request was sent
    /// (e.g. a non-canonical recording id) (→ 400). Carries the message.
    BadRequest(String),
    /// A local failure in the bridge itself, before/without a daemon round-trip
    /// (e.g. reading config for `GET /api/recipes`) (→ 500). Carries the message.
    Internal(String),
}

impl From<IpcTransportError> for RestError {
    fn from(e: IpcTransportError) -> Self {
        RestError::Transport(e)
    }
}

impl From<IpcError> for RestError {
    fn from(e: IpcError) -> Self {
        RestError::Daemon(e)
    }
}

/// Map a daemon [`IpcErrorKind`] onto the closest HTTP status code.
///
/// Factored out as a pure function so the mapping can be unit-tested without
/// constructing a full HTTP response.
pub fn status_for_kind(kind: IpcErrorKind) -> StatusCode {
    match kind {
        // The referenced recording/tag/path doesn't exist.
        IpcErrorKind::NotFound => StatusCode::NOT_FOUND,
        // Caller asked for something the current state/config forbids — a
        // client-correctable input/precondition problem.
        IpcErrorKind::AlreadyRecording
        | IpcErrorKind::NotRecording
        | IpcErrorKind::InvalidConfig => StatusCode::BAD_REQUEST,
        // The daemon is up but a dependency it fronts is not, or it is winding
        // down — retryable from the caller's side.
        IpcErrorKind::WhisperUnreachable
        | IpcErrorKind::WhisperTimeout
        | IpcErrorKind::DaemonNotRunning
        | IpcErrorKind::ShuttingDown => StatusCode::SERVICE_UNAVAILABLE,
        // Anything else is an internal failure on the daemon side.
        IpcErrorKind::HookFailed
        | IpcErrorKind::PipeInUse
        | IpcErrorKind::Io
        | IpcErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl RestError {
    /// The HTTP status this error maps to (also used directly in tests).
    pub fn status(&self) -> StatusCode {
        match self {
            RestError::Transport(_) => StatusCode::SERVICE_UNAVAILABLE,
            RestError::Daemon(e) => status_for_kind(e.kind),
            RestError::BadRequest(_) => StatusCode::BAD_REQUEST,
            RestError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The human-readable message carried to the client in the JSON body.
    pub fn message(&self) -> String {
        match self {
            RestError::Transport(e) => format!("daemon not reachable: {e}"),
            RestError::Daemon(e) => e.message.clone(),
            RestError::BadRequest(m) => m.clone(),
            RestError::Internal(m) => m.clone(),
        }
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> AxumResponse {
        let status = self.status();
        let body = Json(json!({ "error": self.message() }));
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_maps_to_404() {
        assert_eq!(
            status_for_kind(IpcErrorKind::NotFound),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn bad_input_kinds_map_to_400() {
        assert_eq!(
            status_for_kind(IpcErrorKind::InvalidConfig),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for_kind(IpcErrorKind::AlreadyRecording),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for_kind(IpcErrorKind::NotRecording),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn unreachable_and_shutdown_map_to_503() {
        assert_eq!(
            status_for_kind(IpcErrorKind::WhisperUnreachable),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_for_kind(IpcErrorKind::ShuttingDown),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_for_kind(IpcErrorKind::DaemonNotRunning),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn internal_kinds_map_to_500() {
        assert_eq!(
            status_for_kind(IpcErrorKind::Internal),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_for_kind(IpcErrorKind::Io),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_for_kind(IpcErrorKind::HookFailed),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn transport_error_is_503() {
        let err = RestError::Transport(IpcTransportError::Closed);
        assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn bad_request_is_400() {
        let err = RestError::BadRequest("bad id".into());
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        assert_eq!(err.message(), "bad id");
    }

    #[test]
    fn daemon_not_found_error_is_404() {
        let err = RestError::Daemon(IpcError {
            kind: IpcErrorKind::NotFound,
            message: "no such recording".into(),
        });
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
        assert_eq!(err.message(), "no such recording");
    }
}
