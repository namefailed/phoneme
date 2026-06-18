//! The MCP tools and their thin map onto `phoneme-ipc` requests.
//!
//! Per the roadmap this layer is a *translator*, not a brain: each tool's
//! `tools/call` arguments are validated into exactly one [`Request`], the
//! daemon does the work, and the [`phoneme_ipc::Response`] value is rendered back as MCP
//! text content. The request-building is factored into the pure
//! [`build_request`] function so it can be unit-tested without a live daemon
//! (mirroring how `bin/phoneme`'s command tests assert the exact `Request` a
//! subcommand sends).
//!
//! The surface stays in lockstep with the in-tree `phoneme-agent-core` registry
//! (same names, same `Request`s, opposite direction). Beyond the original
//! read-only five it now exposes "act on it" tools (set title/favorite, suggest
//! & list tags, summarize, re-run cleanup, retranscribe, more-like-this, words)
//! and the destructive prune tools (delete a recording / delete a tag).
//!
//! Tools:
//!
//! | tool                | request                          |
//! |---------------------|----------------------------------|
//! | `start_recording`   | [`Request::RecordStart`]         |
//! | `stop_recording`    | [`Request::RecordStop`]          |
//! | `get_transcript`    | [`Request::GetRecording`]        |
//! | `search_recordings` | [`Request::SemanticSearch`]      |
//! | `list_recent`       | [`Request::ListRecordings`]      |
//! | `set_title`         | [`Request::SetRecordingTitle`]   |
//! | `set_favorite`      | [`Request::SetFavorite`]         |
//! | `suggest_tags`      | [`Request::SuggestTags`]         |
//! | `list_tags`         | [`Request::ListAllTags`]         |
//! | `summarize`         | [`Request::RerunSummary`]        |
//! | `rerun_cleanup`     | [`Request::RerunCleanup`]        |
//! | `retranscribe`      | [`Request::RetranscribeRecording`]|
//! | `more_like_this`    | [`Request::MoreLikeThis`]        |
//! | `get_words`         | [`Request::GetWords`]            |
//! | `delete_recording`  | [`Request::DeleteRecording`]     |
//! | `delete_tag`        | [`Request::DeleteTag`]           |

use phoneme_core::{ListFilter, RecordMode, RecordingId};
use phoneme_ipc::Request;
use serde_json::{json, Value};

/// Default number of results for `search_recordings` / `list_recent` when the
/// caller omits `limit`.
const DEFAULT_LIMIT: u64 = 10;

/// A tool invocation that failed *before* (or instead of) reaching the daemon —
/// bad arguments, an unknown tool name. Surfaced to the MCP client as a tool
/// result with `isError: true` (never a transport-level JSON-RPC error), so the
/// calling agent sees a clean, actionable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError(pub String);

impl ToolError {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The JSON-RPC `tools/list` payload: every tool with its input schema.
///
/// Schemas are plain JSON-Schema objects (draft the MCP spec expects); kept in
/// code so they can't drift from [`build_request`].
pub fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "start_recording",
                "description": "Start a new audio recording on the Phoneme daemon. \
                    Returns the new recording id. Fails if a recording or meeting \
                    is already active.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "mode": {
                            "type": "string",
                            "enum": ["oneshot", "hold"],
                            "description": "Stop condition. 'oneshot' (default) \
                                auto-stops on silence; 'hold' records until \
                                stop_recording is called."
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "stop_recording",
                "description": "Stop and finalize the active recording. The audio is \
                    saved and queued for transcription. Fails if nothing is recording.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "get_transcript",
                "description": "Fetch the transcript text for a recording by id. \
                    Returns the transcript, or a note that it is not ready yet.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id (e.g. from list_recent \
                                or search_recordings)."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "search_recordings",
                "description": "Semantic + lexical search over the recording library. \
                    Returns matching recordings with id, title, relevance score, and \
                    a transcript snippet.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural-language search query."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Max results to return (default 10)."
                        }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "name": "list_recent",
                "description": "List the most recent recordings (newest first) with \
                    id, title, status, and a transcript snippet.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Max recordings to return (default 10)."
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "set_title",
                "description": "Set or clear a recording's display title. Provide \
                    'title' to set it; omit or leave it blank to revert to the \
                    auto-generated title.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id (e.g. from list_recent \
                                or search_recordings)."
                        },
                        "title": {
                            "type": "string",
                            "description": "The new title. Omit or leave blank to \
                                return to auto-generation."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "set_favorite",
                "description": "Star or un-star a recording (the Favorites view).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id."
                        },
                        "favorite": {
                            "type": "boolean",
                            "description": "true = starred, false = un-starred."
                        }
                    },
                    "required": ["id", "favorite"],
                    "additionalProperties": false
                }
            },
            {
                "name": "suggest_tags",
                "description": "Run the LLM tag-suggestion step for a recording on \
                    demand (awaits the model). Suggestions land on the recording \
                    for the user to approve.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "list_tags",
                "description": "List every tag in the library (including tags not \
                    attached to any recording).",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "summarize",
                "description": "Generate (or regenerate) and store an LLM summary of \
                    a recording's current transcript.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "rerun_cleanup",
                "description": "Re-run the LLM cleanup step on a recording's \
                    preserved original transcript. Does not re-transcribe the \
                    audio.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "retranscribe",
                "description": "Re-transcribe a saved recording through the full \
                    pipeline. Heavy: this re-runs transcription and post-processing. \
                    Optionally override the transcription 'model' for this run only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id."
                        },
                        "model": {
                            "type": "string",
                            "description": "One-time transcription model override (a \
                                model file path for the local backend, a model id \
                                for cloud backends). Omit to use the configured model."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "more_like_this",
                "description": "Find recordings semantically similar to a stored one, \
                    using its existing vectors (no fresh query embedding). Returns \
                    ranked hits with id, title, score, and a snippet.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording whose stored vectors are \
                                the query."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Max results to return (default 10)."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_words",
                "description": "Fetch a recording's word-level timings (start/end \
                    offsets per word) — e.g. for caption/SRT export.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "delete_recording",
                "description": "Permanently delete a recording (and, by default, its \
                    audio file). Irreversible — confirm with the user before calling.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The recording id."
                        },
                        "keep_audio": {
                            "type": "boolean",
                            "description": "true = remove only the catalog row and \
                                leave the WAV on disk (default false)."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "delete_tag",
                "description": "Delete a tag everywhere, detaching it from every \
                    recording. Irreversible — confirm with the user before calling.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "description": "The tag's id (from list_tags)."
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            }
        ]
    })
}

/// Translate a `tools/call` (name + arguments object) into the single
/// [`Request`] it maps to.
///
/// Pure and daemon-free: argument validation lives here so tests can assert the
/// exact request without standing up a daemon. `arguments` is the raw MCP
/// `arguments` object (may be `null`/absent → treated as empty).
pub fn build_request(name: &str, arguments: &Value) -> Result<Request, ToolError> {
    let args = arguments;
    match name {
        "start_recording" => {
            let mode = match args.get("mode").and_then(Value::as_str) {
                None | Some("oneshot") => RecordMode::Oneshot,
                Some("hold") => RecordMode::Hold,
                Some(other) => {
                    return Err(ToolError::new(format!(
                        "invalid mode '{other}': expected 'oneshot' or 'hold'"
                    )))
                }
            };
            Ok(Request::RecordStart {
                mode,
                in_place: false,
                recipe_id: None,
                whisper_model: None,
            })
        }
        "stop_recording" => Ok(Request::RecordStop),
        "get_transcript" => {
            let id = require_recording_id(args)?;
            Ok(Request::GetRecording { id })
        }
        "search_recordings" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .filter(|q| !q.trim().is_empty())
                .ok_or_else(|| ToolError::new("missing required argument 'query'"))?
                .to_string();
            let limit = optional_limit(args)? as usize;
            Ok(Request::SemanticSearch { query, limit })
        }
        "list_recent" => {
            let limit = optional_limit(args)?;
            Ok(Request::ListRecordings {
                filter: ListFilter {
                    limit: Some(limit as u32),
                    sort_desc: Some(true),
                    ..Default::default()
                },
            })
        }
        "set_title" => {
            let id = require_recording_id(args)?;
            // Some(non-empty) sets a user title; None (omitted or blank) reverts
            // to auto-generation.
            let title = optional_string(args, "title");
            Ok(Request::SetRecordingTitle { id, title })
        }
        "set_favorite" => {
            let id = require_recording_id(args)?;
            let favorite = args
                .get("favorite")
                .and_then(Value::as_bool)
                .ok_or_else(|| ToolError::new("missing required boolean 'favorite'"))?;
            Ok(Request::SetFavorite { id, favorite })
        }
        "suggest_tags" => {
            let id = require_recording_id(args)?;
            Ok(Request::SuggestTags { id })
        }
        "list_tags" => Ok(Request::ListAllTags),
        "summarize" => {
            let id = require_recording_id(args)?;
            Ok(Request::RerunSummary {
                id,
                model: None,
                prompt: None,
            })
        }
        "rerun_cleanup" => {
            let id = require_recording_id(args)?;
            Ok(Request::RerunCleanup {
                id,
                model: None,
                provider: None,
                prompt: None,
                api_url: None,
                api_key: None,
            })
        }
        "retranscribe" => {
            let id = require_recording_id(args)?;
            let model = optional_string(args, "model");
            Ok(Request::RetranscribeRecording {
                id,
                model,
                run_hooks: None,
                post_process: None,
                all_overrides: None,
            })
        }
        "more_like_this" => {
            let id = require_recording_id(args)?;
            let limit = optional_limit(args)? as usize;
            Ok(Request::MoreLikeThis { id, limit })
        }
        "get_words" => {
            let id = require_recording_id(args)?;
            Ok(Request::GetWords { id })
        }
        "delete_recording" => {
            let id = require_recording_id(args)?;
            let keep_audio = args
                .get("keep_audio")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(Request::DeleteRecording { id, keep_audio })
        }
        "delete_tag" => {
            let id = args
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| ToolError::new("missing required integer 'id'"))?;
            Ok(Request::DeleteTag { id })
        }
        other => Err(ToolError::new(format!("unknown tool '{other}'"))),
    }
}

/// Pull the required `id` argument and parse it into a [`RecordingId`].
fn require_recording_id(args: &Value) -> Result<RecordingId, ToolError> {
    let raw = args
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required argument 'id'"))?;
    RecordingId::parse(raw).ok_or_else(|| ToolError::new(format!("invalid recording id '{raw}'")))
}

/// Read an optional string argument, normalized to `Some(non-empty)` or `None`
/// (a missing key or a blank/whitespace-only value both map to `None`). Used by
/// the tools where omitting a field is meaningful — `set_title` (None reverts to
/// auto) and `retranscribe` (None keeps the configured model).
fn optional_string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read an optional `limit` argument (positive integer), defaulting to
/// [`DEFAULT_LIMIT`]. Rejects zero/negative values with a clear message.
fn optional_limit(args: &Value) -> Result<u64, ToolError> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(DEFAULT_LIMIT),
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| ToolError::new("'limit' must be a positive integer"))?;
            if n == 0 {
                return Err(ToolError::new("'limit' must be at least 1"));
            }
            Ok(n)
        }
    }
}

/// Render the daemon's successful [`phoneme_ipc::Response`] value for a tool into the
/// human-readable text an MCP client shows.
///
/// `tool` selects the shaping: ack-style tools echo the new id; the query tools
/// summarize each row (id, title, score, snippet). Unknown shapes fall back to
/// pretty JSON so nothing is ever silently dropped.
pub fn render_result(tool: &str, value: &Value) -> String {
    match tool {
        "start_recording" => match value.get("id").and_then(Value::as_str) {
            Some(id) => format!("Recording started. id: {id}"),
            None => "Recording started.".to_string(),
        },
        "stop_recording" => match value.get("id").and_then(Value::as_str) {
            Some(id) => format!("Recording stopped. id: {id}"),
            None => "Recording stopped.".to_string(),
        },
        "get_transcript" => render_transcript(value),
        "search_recordings" => render_search(value),
        "list_recent" => render_recent(value),
        // `more_like_this` shares `search_recordings`'s `[{recording, score}]`
        // shape, so it renders identically.
        "more_like_this" => render_search(value),
        "list_tags" => render_tags(value),
        "get_words" => render_words(value),
        // The mutating tools all answer Ok `null` (a bare acknowledgement); a
        // short confirmation is the useful rendering.
        "set_title" => "Title updated.".to_string(),
        "set_favorite" => "Favorite updated.".to_string(),
        "suggest_tags" => "Tag suggestions generated.".to_string(),
        "summarize" => "Summary generated.".to_string(),
        "rerun_cleanup" => "Cleanup re-run started.".to_string(),
        "retranscribe" => "Re-transcription started.".to_string(),
        "delete_recording" => "Recording deleted.".to_string(),
        "delete_tag" => "Tag deleted.".to_string(),
        _ => pretty(value),
    }
}

/// `list_tags`: an array of tag objects (`{"id","name","color"}`) → a bulleted
/// list of names.
fn render_tags(value: &Value) -> String {
    let Some(tags) = value.as_array() else {
        return pretty(value);
    };
    if tags.is_empty() {
        return "No tags yet.".to_string();
    }
    let mut out = String::new();
    for tag in tags {
        let name = tag
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unnamed)");
        out.push_str(&format!("- {name}\n"));
    }
    out.trim_end().to_string()
}

/// `get_words`: an array of word objects → a count plus a note (the full timings
/// are large; the agent fetches them when it needs the structured data).
fn render_words(value: &Value) -> String {
    match value.as_array() {
        Some(words) if !words.is_empty() => format!(
            "{} word-level timings available (start/end offsets per word, e.g. for caption/SRT export).",
            words.len()
        ),
        Some(_) => "No word-level timings for this recording yet.".to_string(),
        None => pretty(value),
    }
}

/// `get_transcript`: pull the transcript out of the recording DTO.
fn render_transcript(value: &Value) -> String {
    let id = value.get("id").and_then(Value::as_str).unwrap_or("?");
    match value.get("transcript") {
        Some(Value::String(t)) if !t.is_empty() => t.clone(),
        _ => {
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Recording {id} has no transcript yet (status: {status}).")
        }
    }
}

/// `search_recordings`: each hit is `{recording, score}`.
fn render_search(value: &Value) -> String {
    let Some(hits) = value.as_array() else {
        return pretty(value);
    };
    if hits.is_empty() {
        return "No matching recordings.".to_string();
    }
    let mut out = String::new();
    for hit in hits {
        let score = hit.get("score").and_then(Value::as_f64).unwrap_or(0.0);
        let rec = hit.get("recording").unwrap_or(hit);
        let id = rec.get("id").and_then(Value::as_str).unwrap_or("?");
        let title = display_title(rec);
        let snippet = snippet(rec);
        out.push_str(&format!("[{score:.3}] {id}  {title}\n    {snippet}\n"));
    }
    out.trim_end().to_string()
}

/// `list_recent`: an array of recording DTOs.
fn render_recent(value: &Value) -> String {
    let Some(rows) = value.as_array() else {
        return pretty(value);
    };
    if rows.is_empty() {
        return "No recordings yet.".to_string();
    }
    let mut out = String::new();
    for rec in rows {
        let id = rec.get("id").and_then(Value::as_str).unwrap_or("?");
        let title = display_title(rec);
        let status = rec
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let snippet = snippet(rec);
        out.push_str(&format!("{id}  [{status}]  {title}\n    {snippet}\n"));
    }
    out.trim_end().to_string()
}

/// A recording's display title, falling back to its start timestamp then id.
fn display_title(rec: &Value) -> String {
    if let Some(t) = rec.get("title").and_then(Value::as_str) {
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(ts) = rec.get("started_at").and_then(Value::as_str) {
        return ts.to_string();
    }
    rec.get("id")
        .and_then(Value::as_str)
        .unwrap_or("(untitled)")
        .to_string()
}

/// A short, single-line transcript preview (first ~80 chars), or a placeholder.
fn snippet(rec: &Value) -> String {
    match rec.get("transcript").and_then(Value::as_str) {
        Some(t) if !t.is_empty() => {
            let flat: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
            if flat.chars().count() > 80 {
                let s: String = flat.chars().take(80).collect();
                format!("{s}…")
            } else {
                flat
            }
        }
        _ => "(no transcript)".to_string(),
    }
}

/// Pretty-print a JSON value as a fallback rendering.
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool name this bridge exposes, in `tools/list` order.
    const EXPECTED_TOOLS: [&str; 16] = [
        "start_recording",
        "stop_recording",
        "get_transcript",
        "search_recordings",
        "list_recent",
        "set_title",
        "set_favorite",
        "suggest_tags",
        "list_tags",
        "summarize",
        "rerun_cleanup",
        "retranscribe",
        "more_like_this",
        "get_words",
        "delete_recording",
        "delete_tag",
    ];

    #[test]
    fn tools_list_has_all_tools_with_schemas() {
        let list = tools_list();
        let tools = list["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), EXPECTED_TOOLS.len());
        for t in tools {
            let name = t["name"].as_str().expect("tool name");
            assert!(EXPECTED_TOOLS.contains(&name), "unexpected tool {name}");
            assert!(t["description"].is_string(), "{name} needs a description");
            assert_eq!(
                t["inputSchema"]["type"], "object",
                "{name} inputSchema must be an object schema"
            );
            assert!(
                t["inputSchema"]["properties"].is_object(),
                "{name} inputSchema needs a properties object"
            );
        }
    }

    #[test]
    fn tool_names_match_tools_list() {
        let list = tools_list();
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, EXPECTED_TOOLS);

        // Every advertised name must be dispatchable in `build_request` — a name
        // is only "unknown" when it never appears in `tools_list`. Calling with
        // empty args may fail validation (missing `id`/`query`), but it must
        // never come back as the "unknown tool" sentinel.
        for name in names {
            if let Err(e) = build_request(name, &json!({})) {
                assert!(
                    !e.0.contains("unknown tool"),
                    "{name} is advertised but not dispatchable: {e}"
                );
            }
        }
    }

    #[test]
    fn start_recording_defaults_to_oneshot() {
        let req = build_request("start_recording", &json!({})).unwrap();
        assert_eq!(
            req,
            Request::RecordStart {
                mode: RecordMode::Oneshot,
                in_place: false,
                recipe_id: None,
                whisper_model: None
            }
        );
    }

    #[test]
    fn start_recording_accepts_hold_mode() {
        let req = build_request("start_recording", &json!({"mode": "hold"})).unwrap();
        assert_eq!(
            req,
            Request::RecordStart {
                mode: RecordMode::Hold,
                in_place: false,
                recipe_id: None,
                whisper_model: None
            }
        );
    }

    #[test]
    fn start_recording_rejects_bad_mode() {
        let err = build_request("start_recording", &json!({"mode": "burst"})).unwrap_err();
        assert!(err.0.contains("invalid mode"), "got: {err}");
    }

    #[test]
    fn stop_recording_maps_to_record_stop() {
        assert_eq!(
            build_request("stop_recording", &json!({})).unwrap(),
            Request::RecordStop
        );
    }

    #[test]
    fn get_transcript_requires_valid_id() {
        // A well-formed id parses through to GetRecording.
        let id = RecordingId::new();
        let req = build_request("get_transcript", &json!({"id": id.as_str()})).unwrap();
        assert_eq!(req, Request::GetRecording { id });

        // Missing id → tool error.
        assert!(build_request("get_transcript", &json!({})).is_err());
        // Malformed id → tool error (not a panic).
        assert!(build_request("get_transcript", &json!({"id": "nope"})).is_err());
    }

    #[test]
    fn search_recordings_maps_query_and_limit() {
        let req = build_request(
            "search_recordings",
            &json!({"query": "budget meeting", "limit": 3}),
        )
        .unwrap();
        assert_eq!(
            req,
            Request::SemanticSearch {
                query: "budget meeting".to_string(),
                limit: 3
            }
        );
    }

    #[test]
    fn search_recordings_defaults_limit_and_requires_query() {
        let req = build_request("search_recordings", &json!({"query": "x"})).unwrap();
        assert_eq!(
            req,
            Request::SemanticSearch {
                query: "x".to_string(),
                limit: DEFAULT_LIMIT as usize
            }
        );
        assert!(build_request("search_recordings", &json!({})).is_err());
        assert!(build_request("search_recordings", &json!({"query": "  "})).is_err());
    }

    #[test]
    fn list_recent_builds_newest_first_filter() {
        let req = build_request("list_recent", &json!({"limit": 5})).unwrap();
        match req {
            Request::ListRecordings { filter } => {
                assert_eq!(filter.limit, Some(5));
                assert_eq!(filter.sort_desc, Some(true));
            }
            other => panic!("expected ListRecordings, got {other:?}"),
        }
    }

    #[test]
    fn list_recent_default_limit() {
        let req = build_request("list_recent", &json!({})).unwrap();
        match req {
            Request::ListRecordings { filter } => {
                assert_eq!(filter.limit, Some(DEFAULT_LIMIT as u32));
            }
            other => panic!("expected ListRecordings, got {other:?}"),
        }
    }

    #[test]
    fn limit_zero_is_rejected() {
        assert!(build_request("list_recent", &json!({"limit": 0})).is_err());
        assert!(build_request("search_recordings", &json!({"query": "x", "limit": 0})).is_err());
    }

    #[test]
    fn unknown_tool_is_an_error() {
        let err = build_request("teleport", &json!({})).unwrap_err();
        assert!(err.0.contains("unknown tool"), "got: {err}");
    }

    #[test]
    fn render_start_and_stop_echo_id() {
        assert_eq!(
            render_result("start_recording", &json!({"id": "abc"})),
            "Recording started. id: abc"
        );
        assert_eq!(
            render_result("stop_recording", &json!({"id": "abc"})),
            "Recording stopped. id: abc"
        );
    }

    #[test]
    fn render_transcript_reports_missing_text() {
        let out = render_result(
            "get_transcript",
            &json!({"id": "abc", "status": "transcribing"}),
        );
        assert!(out.contains("no transcript yet"), "got: {out}");
        assert!(out.contains("transcribing"));

        let out = render_result("get_transcript", &json!({"transcript": "hello world"}));
        assert_eq!(out, "hello world");
    }

    #[test]
    fn render_search_summarizes_hits() {
        let value = json!([
            {"score": 0.91, "recording": {"id": "r1", "title": "Standup", "transcript": "we discussed the plan"}},
            {"score": 0.42, "recording": {"id": "r2", "transcript": "another note"}}
        ]);
        let out = render_result("search_recordings", &value);
        assert!(out.contains("0.910"));
        assert!(out.contains("r1"));
        assert!(out.contains("Standup"));
        assert!(out.contains("we discussed the plan"));
    }

    #[test]
    fn render_search_handles_empty() {
        assert_eq!(
            render_result("search_recordings", &json!([])),
            "No matching recordings."
        );
    }

    #[test]
    fn render_recent_lists_rows() {
        let value = json!([
            {"id": "r1", "status": "done", "title": "Demo", "transcript": "alpha beta"}
        ]);
        let out = render_result("list_recent", &value);
        assert!(out.contains("r1"));
        assert!(out.contains("[done]"));
        assert!(out.contains("Demo"));
        assert!(out.contains("alpha beta"));
    }

    // ── New "act on it" tools ────────────────────────────────────────────

    #[test]
    fn set_title_some_vs_none() {
        let id = RecordingId::new();
        // A real title → Some.
        assert_eq!(
            build_request(
                "set_title",
                &json!({"id": id.as_str(), "title": "Budget call"})
            )
            .unwrap(),
            Request::SetRecordingTitle {
                id: id.clone(),
                title: Some("Budget call".to_string())
            }
        );
        // Omitted title → None (revert to auto).
        assert_eq!(
            build_request("set_title", &json!({"id": id.as_str()})).unwrap(),
            Request::SetRecordingTitle {
                id: id.clone(),
                title: None
            }
        );
        // Blank title → None.
        assert_eq!(
            build_request("set_title", &json!({"id": id.as_str(), "title": "   "})).unwrap(),
            Request::SetRecordingTitle { id, title: None }
        );
    }

    #[test]
    fn set_favorite_maps_and_requires_flag() {
        let id = RecordingId::new();
        assert_eq!(
            build_request(
                "set_favorite",
                &json!({"id": id.as_str(), "favorite": true})
            )
            .unwrap(),
            Request::SetFavorite {
                id: id.clone(),
                favorite: true
            }
        );
        // Missing the required boolean → tool error (never reaches the daemon).
        assert!(build_request("set_favorite", &json!({"id": id.as_str()})).is_err());
    }

    #[test]
    fn suggest_tags_and_list_tags_map() {
        let id = RecordingId::new();
        assert_eq!(
            build_request("suggest_tags", &json!({"id": id.as_str()})).unwrap(),
            Request::SuggestTags { id }
        );
        assert_eq!(
            build_request("list_tags", &json!({})).unwrap(),
            Request::ListAllTags
        );
    }

    #[test]
    fn summarize_and_cleanup_default_their_overrides() {
        let id = RecordingId::new();
        assert_eq!(
            build_request("summarize", &json!({"id": id.as_str()})).unwrap(),
            Request::RerunSummary {
                id: id.clone(),
                model: None,
                prompt: None
            }
        );
        assert_eq!(
            build_request("rerun_cleanup", &json!({"id": id.as_str()})).unwrap(),
            Request::RerunCleanup {
                id,
                model: None,
                provider: None,
                prompt: None,
                api_url: None,
                api_key: None,
            }
        );
    }

    #[test]
    fn retranscribe_model_override_some_vs_none() {
        let id = RecordingId::new();
        // No model → None override.
        assert_eq!(
            build_request("retranscribe", &json!({"id": id.as_str()})).unwrap(),
            Request::RetranscribeRecording {
                id: id.clone(),
                model: None,
                run_hooks: None,
                post_process: None,
                all_overrides: None,
            }
        );
        // A model → Some override.
        assert_eq!(
            build_request(
                "retranscribe",
                &json!({"id": id.as_str(), "model": "large-v3"})
            )
            .unwrap(),
            Request::RetranscribeRecording {
                id,
                model: Some("large-v3".to_string()),
                run_hooks: None,
                post_process: None,
                all_overrides: None,
            }
        );
    }

    #[test]
    fn more_like_this_defaults_limit() {
        let id = RecordingId::new();
        assert_eq!(
            build_request("more_like_this", &json!({"id": id.as_str()})).unwrap(),
            Request::MoreLikeThis {
                id: id.clone(),
                limit: DEFAULT_LIMIT as usize
            }
        );
        assert_eq!(
            build_request("more_like_this", &json!({"id": id.as_str(), "limit": 3})).unwrap(),
            Request::MoreLikeThis { id, limit: 3 }
        );
    }

    #[test]
    fn get_words_maps_and_rejects_bad_id() {
        let id = RecordingId::new();
        assert_eq!(
            build_request("get_words", &json!({"id": id.as_str()})).unwrap(),
            Request::GetWords { id }
        );
        // Invalid id → tool error (covers the shared id-parse path; not a panic).
        assert!(build_request("get_words", &json!({"id": "nope"})).is_err());
        // Missing id → tool error too.
        assert!(build_request("get_words", &json!({})).is_err());
    }

    #[test]
    fn render_mutating_tools_confirm() {
        // Ok `null` mutations render a short confirmation.
        assert_eq!(render_result("set_title", &Value::Null), "Title updated.");
        assert_eq!(
            render_result("set_favorite", &Value::Null),
            "Favorite updated."
        );
        assert_eq!(
            render_result("suggest_tags", &Value::Null),
            "Tag suggestions generated."
        );
        assert_eq!(
            render_result("summarize", &Value::Null),
            "Summary generated."
        );
        assert_eq!(
            render_result("rerun_cleanup", &Value::Null),
            "Cleanup re-run started."
        );
        assert_eq!(
            render_result("retranscribe", &Value::Null),
            "Re-transcription started."
        );
    }

    #[test]
    fn render_list_tags_bullets_names() {
        let value = json!([
            {"id": 1, "name": "work", "color": "#4caf50"},
            {"id": 2, "name": "ideas", "color": null}
        ]);
        let out = render_result("list_tags", &value);
        assert!(out.contains("- work"), "got: {out}");
        assert!(out.contains("- ideas"), "got: {out}");
        assert_eq!(render_result("list_tags", &json!([])), "No tags yet.");
    }

    #[test]
    fn render_words_counts() {
        let value = json!([
            {"idx": 0, "start_ms": 0, "end_ms": 100, "text": "hi"},
            {"idx": 1, "start_ms": 100, "end_ms": 200, "text": "there"}
        ]);
        let out = render_result("get_words", &value);
        assert!(out.contains('2'), "got: {out}");
        assert!(
            render_result("get_words", &json!([])).contains("No word-level timings"),
            "empty words should note none yet"
        );
    }

    #[test]
    fn delete_recording_defaults_keep_audio_false() {
        let id = RecordingId::new();
        assert_eq!(
            build_request("delete_recording", &json!({"id": id.as_str()})).unwrap(),
            Request::DeleteRecording {
                id: id.clone(),
                keep_audio: false
            }
        );
        assert_eq!(
            build_request(
                "delete_recording",
                &json!({"id": id.as_str(), "keep_audio": true})
            )
            .unwrap(),
            Request::DeleteRecording {
                id,
                keep_audio: true
            }
        );
        // Bad/missing id → tool error, never the daemon.
        assert!(build_request("delete_recording", &json!({"id": "nope"})).is_err());
        assert!(build_request("delete_recording", &json!({})).is_err());
    }

    #[test]
    fn delete_tag_requires_integer_id() {
        assert_eq!(
            build_request("delete_tag", &json!({"id": 7})).unwrap(),
            Request::DeleteTag { id: 7 }
        );
        assert!(build_request("delete_tag", &json!({})).is_err());
        assert!(build_request("delete_tag", &json!({"id": "nope"})).is_err());
    }

    #[test]
    fn render_delete_tools_confirm() {
        assert_eq!(
            render_result("delete_recording", &Value::Null),
            "Recording deleted."
        );
        assert_eq!(render_result("delete_tag", &Value::Null), "Tag deleted.");
    }

    #[test]
    fn render_more_like_this_reuses_search_shape() {
        let value = json!([
            {"score": 0.77, "recording": {"id": "r9", "title": "Roadmap", "transcript": "ship the agent"}}
        ]);
        let out = render_result("more_like_this", &value);
        assert!(out.contains("0.770"));
        assert!(out.contains("r9"));
        assert!(out.contains("Roadmap"));
    }
}
