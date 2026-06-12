//! "More like this" — the Tauri command behind the recall flow: given a
//! stored recording, ask the daemon for its semantic neighbours.
//!
//! Forwards `Request::MoreLikeThis` over the daemon bridge and returns the
//! same `[{ "recording": …, "score": … }]` array shape as `semantic_search`,
//! so the WebView renders relevance chips for both search paths with one code
//! path. No query embedding happens anywhere on the way — the daemon scores
//! the library against the recording's already-stored vectors, which is what
//! makes the lookup essentially free (and lets it work even while the
//! embedding model isn't loaded).

use crate::bridge::BridgeSlot;
use crate::commands::CommandError;
use phoneme_ipc::{IpcErrorKind, Request, Response};
use serde_json::Value;
use tauri::State;

/// A daemon error kind as its snake_case wire string — the exact string the
/// kind serializes to on the pipe — so the WebView branches on `kind` the
/// same way it does for every other command.
fn kind_str(kind: IpcErrorKind) -> String {
    match serde_json::to_value(kind) {
        Ok(Value::String(s)) => s,
        _ => "internal".into(),
    }
}

/// Find recordings semantically similar to a stored one, using its existing
/// embeddings. The daemon excludes the source recording (and the other track
/// of its own meeting) and returns calibrated 0..1 scores; a recording with
/// no stored vectors yet errors with a clear "isn't indexed yet" message the
/// UI can show as-is.
#[tauri::command]
pub async fn more_like_this(
    bridge: State<'_, BridgeSlot>,
    id: String,
    limit: usize,
) -> Result<Value, CommandError> {
    // Validate the WebView-supplied id up front — a malformed id must fail
    // cleanly here, not deep in the daemon's fixed-offset id accessors.
    let id = phoneme_core::RecordingId::parse(id.as_str()).ok_or_else(|| CommandError {
        kind: "invalid_config".into(),
        message: format!("invalid recording id: {id:?}"),
    })?;
    // Same connect-on-demand behavior as the other commands: an empty slot
    // retries the connect (auto-spawning the daemon) before giving up.
    let bridge = bridge.get_or_connect().await.ok_or_else(|| CommandError {
        kind: "daemon_not_running".into(),
        message: "daemon not reachable; start it with `phoneme daemon --start`".into(),
    })?;
    match bridge.request(Request::MoreLikeThis { id, limit }).await {
        Ok(Response::Ok(v)) => Ok(v),
        Ok(Response::Err(e)) => Err(CommandError {
            kind: kind_str(e.kind),
            message: e.message,
        }),
        Err(e) => Err(CommandError {
            kind: "transport".into(),
            message: format!("transport error: {e}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_str_matches_the_wire_format() {
        // The WebView branches on these strings; they must be the serde wire
        // names, not a hand-rolled mapping that could drift.
        assert_eq!(kind_str(IpcErrorKind::NotFound), "not_found");
        assert_eq!(kind_str(IpcErrorKind::Internal), "internal");
        assert_eq!(kind_str(IpcErrorKind::InvalidConfig), "invalid_config");
    }
}
