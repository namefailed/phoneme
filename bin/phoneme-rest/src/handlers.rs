//! axum handlers: extract params, build the [`phoneme_ipc::Request`] via
//! [`crate::request_map`], forward it to the daemon, and return the daemon's
//! JSON value verbatim.
//!
//! Handlers are deliberately tiny — all the request-shaping lives in
//! [`crate::request_map`] (pure, unit-tested) and all the error→status mapping
//! in [`crate::error`]. A handler's only job is glue: pull the path/query, map
//! a bad `:id` to `400`, call [`crate::daemon::forward`], wrap the result as
//! `Json`.

use axum::extract::{Path, Query, State};
use axum::Json;

use crate::daemon;
use crate::error::RestError;
use crate::request_map::{self, ListQuery, SearchQuery};
use crate::server::AppState;

/// Resolve a `:id` path segment or fail with `400 Bad Request`.
fn require_id(raw: &str) -> Result<phoneme_core::RecordingId, RestError> {
    request_map::parse_id(raw)
        .ok_or_else(|| RestError::BadRequest(format!("'{raw}' is not a valid recording id")))
}

/// `GET /api/recordings?limit=&offset=&kind=` — list the catalog.
pub async fn list_recordings(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, RestError> {
    let value = daemon::forward(&state.pipe_name, request_map::list_recordings(&q)).await?;
    Ok(Json(value))
}

/// `GET /api/recordings/:id` — fetch one recording.
pub async fn get_recording(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    let value = daemon::forward(&state.pipe_name, request_map::get_recording(id)).await?;
    Ok(Json(value))
}

/// `GET /api/recordings/:id/segments` — fetch the recording's transcript
/// segments in timeline order.
pub async fn get_segments(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    let value = daemon::forward(&state.pipe_name, request_map::get_segments(id)).await?;
    Ok(Json(value))
}

/// `GET /api/search?q=&limit=` — hybrid semantic + lexical recall.
pub async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, RestError> {
    let value = daemon::forward(&state.pipe_name, request_map::search(&q)).await?;
    Ok(Json(value))
}

/// `POST /api/record/start` — start a `hold`-mode recording.
pub async fn record_start(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RestError> {
    let value = daemon::forward(&state.pipe_name, request_map::record_start()).await?;
    Ok(Json(value))
}

/// `POST /api/record/stop` — stop and finalize the active recording.
pub async fn record_stop(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RestError> {
    let value = daemon::forward(&state.pipe_name, request_map::record_stop()).await?;
    Ok(Json(value))
}

/// `GET /api/status` — the daemon's liveness + identity probe.
pub async fn status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, RestError> {
    let value = daemon::forward(&state.pipe_name, request_map::daemon_status()).await?;
    Ok(Json(value))
}

/// `GET /api/health` — `200 {"status":"ok"}` if the daemon answered a
/// `DaemonStatus` probe, otherwise the usual `503` (the daemon is the only
/// dependency, so its reachability *is* the health of the bridge).
pub async fn health(State(state): State<AppState>) -> Result<Json<serde_json::Value>, RestError> {
    daemon::forward(&state.pipe_name, request_map::daemon_status()).await?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}
