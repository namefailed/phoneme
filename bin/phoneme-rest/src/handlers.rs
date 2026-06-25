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
use phoneme_ipc::Request;

use crate::daemon;
use crate::error::RestError;
use crate::request_map::{
    self, AttachTagBody, ClipBody, FavoriteBody, ImportBody, ListQuery, PinnedBody, SearchQuery,
    SimilarQuery, TitleBody,
};
use crate::server::AppState;

/// Resolve a `:id` path segment or fail with `400 Bad Request`.
fn require_id(raw: &str) -> Result<phoneme_core::RecordingId, RestError> {
    request_map::parse_id(raw)
        .ok_or_else(|| RestError::BadRequest(format!("'{raw}' is not a valid recording id")))
}

/// The shared tail of every handler: forward the built `req` to the daemon and
/// wrap its JSON reply as `Json`. Handlers shape the [`Request`] (and, where a
/// `:id` is in the path, validate it first) and hand it here.
async fn forward(state: &AppState, req: Request) -> Result<Json<serde_json::Value>, RestError> {
    Ok(Json(daemon::forward(&state.pipe_name, req).await?))
}

/// `GET /api/recordings?limit=&offset=&kind=` — list the catalog.
pub async fn list_recordings(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::list_recordings(&q)).await
}

/// `GET /api/recordings/:id` — fetch one recording.
pub async fn get_recording(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::get_recording(id)).await
}

/// `GET /api/recordings/:id/segments` — fetch the recording's transcript
/// segments in timeline order.
pub async fn get_segments(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::get_segments(id)).await
}

/// `GET /api/recordings/:id/words` — fetch the per-word layer beneath
/// `segments` (word seek, confidence). May be an empty array — a normal state.
pub async fn get_words(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::get_words(id)).await
}

/// `GET /api/recordings/:id/chapters` — fetch the recording's auto-chapters in
/// chronological order. May be an empty array — a normal state (no timing to
/// chapter, or the auto-chapter step never ran).
pub async fn get_chapters(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::get_chapters(id)).await
}

/// `GET /api/recordings/:id/versions` — the compounding transcript-version chain
/// (raw ASR → each step → live) for side-by-side compare. A cross-platform HTTP
/// alternative to the pipe-only access (no named-pipe path needed).
pub async fn get_versions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::transcript_versions(id)).await
}

/// `POST /api/recordings/:id/clip` — export a WAV slice of the recording for a
/// `{start_ms,end_ms[,out_path]}` range; returns `{"path": "…"}`.
pub async fn export_clip(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ClipBody>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::export_clip(id, &body)).await
}

/// `GET /api/recordings/:id/similar?limit=` — "more like this" using the
/// recording's stored vectors as the query.
pub async fn more_like_this(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SimilarQuery>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::more_like_this(id, &q)).await
}

/// `GET /api/search?q=&limit=` — hybrid semantic + lexical recall.
pub async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::search(&q)).await
}

/// `GET /api/tags` — tags attached to at least one recording.
pub async fn list_tags(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::list_tags()).await
}

/// `GET /api/recordings/:id/tags` — the tags attached to one recording.
pub async fn tags_for(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::tags_for(id)).await
}

/// `GET /api/queue` — the transcription pipeline queue (processing first).
pub async fn list_queue(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::list_queue()).await
}

/// `POST /api/recordings/:id/title` — set (`{"title":"…"}`) or clear
/// (`{}`/`{"title":null}`) a recording's display title.
pub async fn set_title(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TitleBody>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::set_title(id, &body)).await
}

/// `POST /api/recordings/:id/favorite` — set/clear the star flag
/// (`{"favorite":true|false}`).
pub async fn set_favorite(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<FavoriteBody>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::set_favorite(id, &body)).await
}

/// `POST /api/recordings/:id/pinned` — set/clear the pinned flag
/// (`{"pinned":true|false}`). Pinned recordings sort to the top of the library.
pub async fn set_pinned(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PinnedBody>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::set_pinned(id, &body)).await
}

/// `POST /api/recordings/:id/tags` — attach an existing tag
/// (`{"tag_id":<id>}`).
pub async fn attach_tag(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AttachTagBody>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::attach_tag(id, &body)).await
}

/// `DELETE /api/recordings/:id/tags/:tag_id` — detach a tag from a recording.
pub async fn detach_tag(
    State(state): State<AppState>,
    Path((id, tag_id)): Path<(String, i64)>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::detach_tag(id, tag_id)).await
}

/// `POST /api/recordings/:id/cleanup` — re-run the LLM cleanup step against the
/// stored original transcript (configured provider/model/prompt).
pub async fn rerun_cleanup(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::rerun_cleanup(id)).await
}

/// `POST /api/recordings/:id/summary` — generate/regenerate the LLM summary of
/// the recording's current transcript.
pub async fn rerun_summary(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let id = require_id(&id)?;
    forward(&state, request_map::rerun_summary(id)).await
}

/// `POST /api/meeting/start` — start a dual-track meeting recording.
pub async fn meeting_start(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::meeting_start()).await
}

/// `POST /api/meeting/stop` — stop and finalize the active meeting.
pub async fn meeting_stop(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::meeting_stop()).await
}

/// `POST /api/record/start` — start a `hold`-mode recording.
pub async fn record_start(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::record_start()).await
}

/// `POST /api/record/stop` — stop and finalize the active recording.
pub async fn record_stop(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::record_stop()).await
}

/// `POST /api/import` — queue a local audio file through the pipeline, optionally
/// under a one-time Playbook recipe (`{"path":"…","recipe_id":"…"}`). The daemon
/// resolves the path on its side; URL/yt-dlp import stays CLI-only.
pub async fn import_recording(
    State(state): State<AppState>,
    Json(body): Json<ImportBody>,
) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::import_recording(&body)).await
}

/// `GET /api/recipes` — the configured Playbook recipes (id, name, description,
/// builtin, scope, steps), so an HTTP client can build a recipe picker (e.g.
/// filter `scope == "recording"` to pair with `POST /api/import`).
///
/// ponytail: recipes live in config, not daemon runtime state, so this reads the
/// same config the daemon does rather than adding an IPC verb just to relay it.
/// Re-reads per call (cheap, rarely hit) so an edited recipe shows up without a
/// bridge restart.
pub async fn list_recipes() -> Result<Json<serde_json::Value>, RestError> {
    let cfg = phoneme_core::Config::load_resolved()
        .map_err(|e| RestError::Internal(format!("failed to load config: {e}")))?;
    serde_json::to_value(&cfg.recipes)
        .map(Json)
        .map_err(|e| RestError::Internal(format!("failed to serialize recipes: {e}")))
}

/// `GET /api/status` — the daemon's liveness + identity probe.
pub async fn status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, RestError> {
    forward(&state, request_map::daemon_status()).await
}

/// `GET /api/health` — `200 {"status":"ok"}` if the daemon answered a
/// `DaemonStatus` probe, otherwise the usual `503` (the daemon is the only
/// dependency, so its reachability *is* the health of the bridge).
pub async fn health(State(state): State<AppState>) -> Result<Json<serde_json::Value>, RestError> {
    daemon::forward(&state.pipe_name, request_map::daemon_status()).await?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}
