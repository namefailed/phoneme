//! `phoneme-agent-core` — the in-tree **tool seam** for Phoneme's embedded agent.
//!
//! This is the compiler-enforced "tool layer" over the daemon's [`Request`]
//! enum: each [`Tool`] declares a name + JSON schema and maps validated JSON
//! arguments to a typed `Request`. **Execution** (sending that request over a
//! `phoneme_ipc::Transport` and rendering the `Response`) is the caller's job —
//! keeping this layer pure, synchronous, and trivially testable, and keeping the
//! tool list in lockstep with the wire contract *at compile time* (a renamed or
//! removed `Request` variant breaks the build here, not at runtime).
//!
//! This crate is the **single source of truth** for the tool catalog. The future
//! in-app agent panel drives this registry directly; the standalone opencode-based
//! agent reaches the same capabilities from outside via the `phoneme-mcp` server,
//! which builds its `tools/list` and dispatches `tools/call` *from this registry*
//! — "same registry, opposite direction" (see
//! `docs/design/phoneme-agent-harness.md`). There is no second hand-maintained
//! tool list: the same tool names map to the same `Request`s in both directions.
//! Beyond the original five read-only tools (list/search/get/start/stop) it
//! exposes "act on it" capabilities — set title/favorite, suggest & list tags,
//! summarize, re-run cleanup, retranscribe, more-like-this, per-word timings, and
//! the destructive prune tools (delete a recording / delete a tag).

use phoneme_core::{ListFilter, RecordMode, RecordingId};
use phoneme_ipc::Request;
use serde_json::{json, Value};

/// Why a tool call could not be turned into a `Request`.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// No tool with that name is registered.
    #[error("unknown tool: {0}")]
    Unknown(String),
    /// The arguments didn't satisfy the tool's schema.
    #[error("invalid arguments for {tool}: {reason}")]
    BadArgs { tool: String, reason: String },
}

/// What a tool advertises to the agent/host: a name, a one-line description, and
/// a JSON-Schema object describing its arguments.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// A typed wrapper that turns validated JSON arguments into a daemon `Request`.
pub trait Tool: Send + Sync {
    /// The tool's advertised name/description/schema.
    fn spec(&self) -> ToolSpec;
    /// Validate `args` and produce the `Request` to send. Pure: no I/O.
    fn to_request(&self, args: &Value) -> Result<Request, ToolError>;
}

/// The set of tools the agent can call.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// The canonical Phoneme toolset — the one source of truth `phoneme-mcp`
    /// exposes externally, in the same order. The read-only core (list / search /
    /// get / start / stop) plus the "act on it" tools (set title/favorite,
    /// suggest & list tags, summarize, re-run cleanup, retranscribe, more-like-
    /// this, words) and the destructive prune tools (delete recording / tag).
    pub fn with_phoneme_tools() -> Self {
        let mut r = Self::new();
        r.register(Box::new(StartRecording));
        r.register(Box::new(StopRecording));
        r.register(Box::new(GetTranscript));
        r.register(Box::new(SearchRecordings));
        r.register(Box::new(ListRecent));
        r.register(Box::new(SetTitle));
        r.register(Box::new(SetFavorite));
        r.register(Box::new(SuggestTags));
        r.register(Box::new(ListTags));
        r.register(Box::new(Summarize));
        r.register(Box::new(RerunCleanup));
        r.register(Box::new(Retranscribe));
        r.register(Box::new(MoreLikeThis));
        r.register(Box::new(GetWords));
        r.register(Box::new(DeleteRecordingTool));
        r.register(Box::new(DeleteTagTool));
        r
    }

    /// Add a tool (e.g. a host-specific extension).
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Every registered tool's spec — the "tools/list" surface.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|t| t.spec()).collect()
    }

    /// Map a named tool call to its `Request`, or an error the host can surface.
    pub fn to_request(&self, name: &str, args: &Value) -> Result<Request, ToolError> {
        self.tools
            .iter()
            .find(|t| t.spec().name == name)
            .ok_or_else(|| ToolError::Unknown(name.to_string()))?
            .to_request(args)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::with_phoneme_tools()
    }
}

/// Default result cap for the list/search tools when the caller omits `limit`.
const DEFAULT_LIMIT: u32 = 10;

fn require_str(args: &Value, key: &str, tool: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ToolError::BadArgs {
            tool: tool.to_string(),
            reason: format!("missing required string `{key}`"),
        })
}

/// Read an optional `limit` argument (positive integer), defaulting to
/// [`DEFAULT_LIMIT`]. Rejects zero / negative / non-integer values with a clear
/// `BadArgs` message rather than silently coercing them.
fn opt_limit(args: &Value, tool: &str) -> Result<u32, ToolError> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(DEFAULT_LIMIT),
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| ToolError::BadArgs {
                tool: tool.to_string(),
                reason: "`limit` must be a positive integer".to_string(),
            })?;
            if n == 0 {
                return Err(ToolError::BadArgs {
                    tool: tool.to_string(),
                    reason: "`limit` must be at least 1".to_string(),
                });
            }
            Ok(n as u32)
        }
    }
}

/// Pull the required `id` argument and parse it into a [`RecordingId`] — the
/// shared validation every id-taking tool uses (same shape as `get_transcript`).
fn require_recording_id(args: &Value, tool: &str) -> Result<RecordingId, ToolError> {
    let raw = require_str(args, "id", tool)?;
    RecordingId::parse(raw).ok_or_else(|| ToolError::BadArgs {
        tool: tool.to_string(),
        reason: "`id` is not a valid recording id".to_string(),
    })
}

/// Pull a required integer argument (e.g. a tag id), or a `BadArgs` error.
fn require_i64(args: &Value, key: &str, tool: &str) -> Result<i64, ToolError> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ToolError::BadArgs {
            tool: tool.to_string(),
            reason: format!("missing required integer `{key}`"),
        })
}

/// Read an optional string argument, normalized to `Some(non-empty)` or `None`
/// (a missing key or a blank/whitespace-only value both map to `None`).
fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

struct StartRecording;
impl Tool for StartRecording {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "start_recording",
            description: "Start a new audio recording on the Phoneme daemon. \
                Returns the new recording id. Fails if a recording or meeting \
                is already active.",
            input_schema: json!({
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
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let mode = match args.get("mode").and_then(|v| v.as_str()) {
            Some("hold") => RecordMode::Hold,
            Some("oneshot") | None => RecordMode::Oneshot,
            Some(other) => {
                return Err(ToolError::BadArgs {
                    tool: "start_recording".to_string(),
                    reason: format!("invalid mode '{other}': expected 'oneshot' or 'hold'"),
                })
            }
        };
        Ok(Request::RecordStart {
            mode,
            in_place: false,
            recipe_id: None,
            whisper_model: None,
            source: None,
        })
    }
}

struct StopRecording;
impl Tool for StopRecording {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "stop_recording",
            description: "Stop and finalize the active recording. The audio is \
                saved and queued for transcription. Fails if nothing is recording.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, _args: &Value) -> Result<Request, ToolError> {
        Ok(Request::RecordStop)
    }
}

struct GetTranscript;
impl Tool for GetTranscript {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_transcript",
            description: "Fetch the transcript text for a recording by id. \
                Returns the transcript, or a note that it is not ready yet.",
            input_schema: json!({
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
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "get_transcript")?;
        Ok(Request::GetRecording { id })
    }
}

struct SearchRecordings;
impl Tool for SearchRecordings {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_recordings",
            description: "Semantic + lexical search over the recording library. \
                Returns matching recordings with id, title, relevance score, and \
                a transcript snippet.",
            input_schema: json!({
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
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let query = require_str(args, "query", "search_recordings")?;
        let limit = opt_limit(args, "search_recordings")? as usize;
        Ok(Request::SemanticSearch { query, limit })
    }
}

struct ListRecent;
impl Tool for ListRecent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_recent",
            description: "List the most recent recordings (newest first) with \
                id, title, status, and a transcript snippet.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Max recordings to return (default 10)."
                    }
                },
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let limit = opt_limit(args, "list_recent")?;
        Ok(Request::ListRecordings {
            filter: ListFilter {
                limit: Some(limit),
                sort_desc: Some(true),
                ..Default::default()
            },
        })
    }
}

struct SetTitle;
impl Tool for SetTitle {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_title",
            description: "Set or clear a recording's display title. Provide \
                'title' to set it; omit or leave it blank to revert to the \
                auto-generated title.",
            input_schema: json!({
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
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "set_title")?;
        // Some(non-empty) sets a user title; None (omitted or blank) reverts to
        // auto-generation.
        let title = opt_str(args, "title");
        Ok(Request::SetRecordingTitle { id, title })
    }
}

struct SetFavorite;
impl Tool for SetFavorite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_favorite",
            description: "Star or un-star a recording (the Favorites view).",
            input_schema: json!({
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
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "set_favorite")?;
        let favorite = args
            .get("favorite")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| ToolError::BadArgs {
                tool: "set_favorite".to_string(),
                reason: "missing required boolean `favorite`".to_string(),
            })?;
        Ok(Request::SetFavorite { id, favorite })
    }
}

struct SuggestTags;
impl Tool for SuggestTags {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "suggest_tags",
            description: "Run the LLM tag-suggestion step for a recording on \
                demand (awaits the model). Suggestions land on the recording \
                for the user to approve.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording id."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "suggest_tags")?;
        Ok(Request::SuggestTags { id })
    }
}

struct ListTags;
impl Tool for ListTags {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_tags",
            description: "List every tag in the library (including tags not \
                attached to any recording).",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, _args: &Value) -> Result<Request, ToolError> {
        Ok(Request::ListAllTags)
    }
}

struct Summarize;
impl Tool for Summarize {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "summarize",
            description: "Generate (or regenerate) and store an LLM summary of \
                a recording's current transcript.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording id."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "summarize")?;
        Ok(Request::RerunSummary {
            id,
            model: None,
            prompt: None,
        })
    }
}

struct RerunCleanup;
impl Tool for RerunCleanup {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rerun_cleanup",
            description: "Re-run the LLM cleanup step on a recording's \
                preserved original transcript. Does not re-transcribe the \
                audio.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording id."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "rerun_cleanup")?;
        Ok(Request::RerunCleanup {
            id,
            model: None,
            provider: None,
            prompt: None,
            api_url: None,
            api_key: None,
        })
    }
}

struct Retranscribe;
impl Tool for Retranscribe {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "retranscribe",
            description: "Re-transcribe a saved recording through the full \
                pipeline. Heavy: this re-runs transcription and post-processing. \
                Optionally override the transcription 'model' for this run only.",
            input_schema: json!({
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
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "retranscribe")?;
        let model = opt_str(args, "model");
        Ok(Request::RetranscribeRecording {
            id,
            model,
            run_hooks: None,
            post_process: None,
            all_overrides: None,
            recipe_id: None,
        })
    }
}

struct MoreLikeThis;
impl Tool for MoreLikeThis {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "more_like_this",
            description: "Find recordings semantically similar to a stored one, \
                using its existing vectors (no fresh query embedding). Returns \
                ranked hits with id, title, score, and a snippet.",
            input_schema: json!({
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
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "more_like_this")?;
        let limit = opt_limit(args, "more_like_this")? as usize;
        Ok(Request::MoreLikeThis { id, limit })
    }
}

struct GetWords;
impl Tool for GetWords {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_words",
            description: "Fetch a recording's word-level timings (start/end \
                offsets per word) — e.g. for caption/SRT export.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording id."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "get_words")?;
        Ok(Request::GetWords { id })
    }
}

struct DeleteRecordingTool;
impl Tool for DeleteRecordingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delete_recording",
            description: "Permanently delete a recording (and, by default, its \
                audio file). Irreversible — confirm with the user before calling.",
            input_schema: json!({
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
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "delete_recording")?;
        let keep_audio = args
            .get("keep_audio")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(Request::DeleteRecording { id, keep_audio })
    }
}

struct DeleteTagTool;
impl Tool for DeleteTagTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delete_tag",
            description: "Delete a tag everywhere, detaching it from every \
                recording. Irreversible — confirm with the user before calling.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "integer",
                        "description": "The tag's id (from list_tags)."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_i64(args, "id", "delete_tag")?;
        Ok(Request::DeleteTag { id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_the_phoneme_tools_in_order() {
        let r = ToolRegistry::with_phoneme_tools();
        let names: Vec<&str> = r.specs().iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
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
            ]
        );
        // Every spec carries an object schema with a properties object.
        assert!(r.specs().iter().all(|s| s.input_schema["type"] == "object"));
        assert!(r
            .specs()
            .iter()
            .all(|s| s.input_schema["properties"].is_object()));
    }

    #[test]
    fn list_recent_defaults_to_ten_and_sorts_newest_first() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("list_recent", &json!({})).unwrap(),
            Request::ListRecordings {
                filter: ListFilter {
                    limit: Some(10),
                    sort_desc: Some(true),
                    ..Default::default()
                }
            }
        );
        assert_eq!(
            r.to_request("list_recent", &json!({ "limit": 3 })).unwrap(),
            Request::ListRecordings {
                filter: ListFilter {
                    limit: Some(3),
                    sort_desc: Some(true),
                    ..Default::default()
                }
            }
        );
    }

    #[test]
    fn search_requires_query_and_defaults_limit() {
        let r = ToolRegistry::with_phoneme_tools();
        assert!(matches!(
            r.to_request("search_recordings", &json!({})),
            Err(ToolError::BadArgs { .. })
        ));
        assert_eq!(
            r.to_request("search_recordings", &json!({ "query": "api redesign" }))
                .unwrap(),
            Request::SemanticSearch {
                query: "api redesign".to_string(),
                limit: 10
            }
        );
    }

    #[test]
    fn limit_zero_and_non_integer_are_rejected() {
        let r = ToolRegistry::with_phoneme_tools();
        assert!(matches!(
            r.to_request("list_recent", &json!({ "limit": 0 })),
            Err(ToolError::BadArgs { .. })
        ));
        assert!(matches!(
            r.to_request("search_recordings", &json!({ "query": "x", "limit": 0 })),
            Err(ToolError::BadArgs { .. })
        ));
        assert!(matches!(
            r.to_request("list_recent", &json!({ "limit": "lots" })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn get_transcript_validates_the_id() {
        let r = ToolRegistry::with_phoneme_tools();
        assert!(matches!(
            r.to_request("get_transcript", &json!({ "id": "not a real id" })),
            Err(ToolError::BadArgs { .. })
        ));
        let id = RecordingId::new();
        assert_eq!(
            r.to_request("get_transcript", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::GetRecording { id }
        );
    }

    #[test]
    fn start_recording_maps_and_validates_mode() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("start_recording", &json!({})).unwrap(),
            Request::RecordStart {
                mode: RecordMode::Oneshot,
                in_place: false,
                recipe_id: None,
                whisper_model: None,
                source: None,
            }
        );
        assert_eq!(
            r.to_request("start_recording", &json!({ "mode": "hold" }))
                .unwrap(),
            Request::RecordStart {
                mode: RecordMode::Hold,
                in_place: false,
                recipe_id: None,
                whisper_model: None,
                source: None,
            }
        );
        assert!(matches!(
            r.to_request("start_recording", &json!({ "mode": "weird" })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn stop_is_unit_and_unknown_tool_errors() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("stop_recording", &json!({})).unwrap(),
            Request::RecordStop
        );
        assert!(matches!(
            r.to_request("nope", &json!({})),
            Err(ToolError::Unknown(_))
        ));
    }

    #[test]
    fn set_title_some_vs_none() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        // A real title → Some.
        assert_eq!(
            r.to_request(
                "set_title",
                &json!({ "id": id.as_str(), "title": "Budget call" })
            )
            .unwrap(),
            Request::SetRecordingTitle {
                id: id.clone(),
                title: Some("Budget call".to_string())
            }
        );
        // Omitted title → None (revert to auto).
        assert_eq!(
            r.to_request("set_title", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::SetRecordingTitle {
                id: id.clone(),
                title: None
            }
        );
        // Blank title → None.
        assert_eq!(
            r.to_request("set_title", &json!({ "id": id.as_str(), "title": "   " }))
                .unwrap(),
            Request::SetRecordingTitle { id, title: None }
        );
    }

    #[test]
    fn set_favorite_maps_and_requires_flag() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request(
                "set_favorite",
                &json!({ "id": id.as_str(), "favorite": true })
            )
            .unwrap(),
            Request::SetFavorite {
                id: id.clone(),
                favorite: true
            }
        );
        // Missing the required boolean → BadArgs.
        assert!(matches!(
            r.to_request("set_favorite", &json!({ "id": id.as_str() })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn suggest_tags_and_list_tags_map() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request("suggest_tags", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::SuggestTags { id }
        );
        assert_eq!(
            r.to_request("list_tags", &json!({})).unwrap(),
            Request::ListAllTags
        );
    }

    #[test]
    fn summarize_and_cleanup_default_their_overrides() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request("summarize", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::RerunSummary {
                id: id.clone(),
                model: None,
                prompt: None
            }
        );
        assert_eq!(
            r.to_request("rerun_cleanup", &json!({ "id": id.as_str() }))
                .unwrap(),
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
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        // No model → None override.
        assert_eq!(
            r.to_request("retranscribe", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::RetranscribeRecording {
                id: id.clone(),
                model: None,
                run_hooks: None,
                post_process: None,
                all_overrides: None,
                recipe_id: None,
            }
        );
        // A model → Some override.
        assert_eq!(
            r.to_request(
                "retranscribe",
                &json!({ "id": id.as_str(), "model": "large-v3" })
            )
            .unwrap(),
            Request::RetranscribeRecording {
                id,
                model: Some("large-v3".to_string()),
                run_hooks: None,
                post_process: None,
                all_overrides: None,
                recipe_id: None,
            }
        );
    }

    #[test]
    fn more_like_this_defaults_limit() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request("more_like_this", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::MoreLikeThis {
                id: id.clone(),
                limit: 10
            }
        );
        assert_eq!(
            r.to_request("more_like_this", &json!({ "id": id.as_str(), "limit": 3 }))
                .unwrap(),
            Request::MoreLikeThis { id, limit: 3 }
        );
    }

    #[test]
    fn get_words_maps_and_validates_id() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request("get_words", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::GetWords { id }
        );
        // Invalid id → BadArgs (covers the new id-taking tools' shared path).
        assert!(matches!(
            r.to_request("get_words", &json!({ "id": "not-an-id" })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn delete_recording_defaults_keep_audio_false() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        // No keep_audio → false (delete the WAV too).
        assert_eq!(
            r.to_request("delete_recording", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::DeleteRecording {
                id: id.clone(),
                keep_audio: false
            }
        );
        // keep_audio: true → keep the WAV.
        assert_eq!(
            r.to_request(
                "delete_recording",
                &json!({ "id": id.as_str(), "keep_audio": true })
            )
            .unwrap(),
            Request::DeleteRecording {
                id,
                keep_audio: true
            }
        );
    }

    #[test]
    fn delete_tag_requires_integer_id() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("delete_tag", &json!({ "id": 7 })).unwrap(),
            Request::DeleteTag { id: 7 }
        );
        // Missing / non-integer id → BadArgs.
        assert!(matches!(
            r.to_request("delete_tag", &json!({})),
            Err(ToolError::BadArgs { .. })
        ));
        assert!(matches!(
            r.to_request("delete_tag", &json!({ "id": "nope" })),
            Err(ToolError::BadArgs { .. })
        ));
    }
}
