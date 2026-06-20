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
//! the destructive prune tools (delete a recording / delete a tag) — plus a
//! meetings + speakers batch: start/stop a meeting, list a meeting's tracks, read
//! a recording's timeline segments, approve/dismiss a suggested tag, name /
//! reassign / merge / split a diarized speaker, recognize named speakers, and
//! manage the named-voice library (list / rename / merge / forget).

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
        r.register(Box::new(GetSegments));
        r.register(Box::new(ApproveTagSuggestion));
        r.register(Box::new(DismissTagSuggestion));
        // Meetings.
        r.register(Box::new(StartMeeting));
        r.register(Box::new(StopMeeting));
        r.register(Box::new(ListMeeting));
        // Speaker correction + recognition.
        r.register(Box::new(SetSpeakerName));
        r.register(Box::new(ReassignSpeakerSegment));
        r.register(Box::new(MergeSpeakers));
        r.register(Box::new(SplitSpeaker));
        r.register(Box::new(RecognizeSpeakers));
        // Named-voice library.
        r.register(Box::new(ListNamedVoices));
        r.register(Box::new(RenameNamedVoice));
        r.register(Box::new(MergeNamedVoices));
        r.register(Box::new(ForgetNamedVoice));
        // Destructive prune tools stay last.
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

/// Require a string argument that *may* be blank — the key must be present and a
/// string, but an empty/whitespace value is meaningful (e.g. `set_speaker_name`
/// uses a blank `name` to clear a label). Returned trimmed.
fn require_present_str(args: &Value, key: &str, tool: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| ToolError::BadArgs {
            tool: tool.to_string(),
            reason: format!("missing required string `{key}`"),
        })
}

/// Require an integer argument that must be at least `min` (e.g. a 1-based speaker
/// label, or a 0-based segment index). Rejects a missing/non-integer/too-small
/// value with a clear `BadArgs` message.
fn require_i64_min(args: &Value, key: &str, min: i64, tool: &str) -> Result<i64, ToolError> {
    let n = require_i64(args, key, tool)?;
    if n < min {
        return Err(ToolError::BadArgs {
            tool: tool.to_string(),
            reason: format!("`{key}` must be at least {min}"),
        });
    }
    Ok(n)
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
        Ok(Request::SemanticSearch {
            query,
            limit,
            filter: None,
        })
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

struct GetSegments;
impl Tool for GetSegments {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_segments",
            description: "Fetch a recording's transcript segments in timeline \
                order (start/end offsets, text, and the diarized speaker label \
                per segment) — e.g. for caption export or the timeline view.",
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
        let id = require_recording_id(args, "get_segments")?;
        Ok(Request::GetSegments { id })
    }
}

struct ApproveTagSuggestion;
impl Tool for ApproveTagSuggestion {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "approve_tag_suggestion",
            description: "Approve one of a recording's suggested tags by name: \
                create the tag if needed, attach it, and drop it from the \
                suggestion list.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording carrying the suggestion."
                    },
                    "name": {
                        "type": "string",
                        "description": "The suggested tag name to approve \
                            (case-insensitive)."
                    }
                },
                "required": ["id", "name"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "approve_tag_suggestion")?;
        let name = require_str(args, "name", "approve_tag_suggestion")?;
        Ok(Request::ApproveTagSuggestion { id, name })
    }
}

struct DismissTagSuggestion;
impl Tool for DismissTagSuggestion {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "dismiss_tag_suggestion",
            description: "Dismiss one of a recording's suggested tags by name \
                (drop it from the suggestion list without attaching it).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording carrying the suggestion."
                    },
                    "name": {
                        "type": "string",
                        "description": "The suggested tag name to dismiss \
                            (case-insensitive)."
                    }
                },
                "required": ["id", "name"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "dismiss_tag_suggestion")?;
        let name = require_str(args, "name", "dismiss_tag_suggestion")?;
        Ok(Request::DismissTagSuggestion { id, name })
    }
}

struct StartMeeting;
impl Tool for StartMeeting {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "start_meeting",
            description: "Start a meeting recording (microphone + system-audio \
                tracks on a shared timeline). Returns the new meeting id. Fails \
                if a recording or meeting is already active.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, _args: &Value) -> Result<Request, ToolError> {
        Ok(Request::StartMeeting)
    }
}

struct StopMeeting;
impl Tool for StopMeeting {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "stop_meeting",
            description: "Stop the active meeting: both tracks are finalized and \
                queued for transcription. Fails if no meeting is active.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, _args: &Value) -> Result<Request, ToolError> {
        Ok(Request::StopMeeting)
    }
}

struct ListMeeting;
impl Tool for ListMeeting {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_meeting",
            description: "List every recording (track) belonging to one meeting \
                session, ordered by track then time.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "meeting_id": {
                        "type": "string",
                        "description": "The shared meeting id both tracks carry \
                            (a recording DTO's `meeting_id`)."
                    }
                },
                "required": ["meeting_id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let meeting_id = require_str(args, "meeting_id", "list_meeting")?;
        Ok(Request::ListMeeting { meeting_id })
    }
}

struct SetSpeakerName;
impl Tool for SetSpeakerName {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_speaker_name",
            description: "Set (or clear) the display name for one diarized \
                speaker of a recording. A blank `name` reverts the label to the \
                default 'Speaker N'. The stored transcript is never rewritten — \
                names apply at display/export time — so a rename is reversible.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording whose speaker map to edit."
                    },
                    "speaker_label": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based index matching the [Speaker N] \
                            transcript marker."
                    },
                    "name": {
                        "type": "string",
                        "description": "The display name; blank clears the mapping."
                    }
                },
                "required": ["id", "speaker_label", "name"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "set_speaker_name")?;
        let speaker_label = require_i64_min(args, "speaker_label", 1, "set_speaker_name")?;
        // A blank name is meaningful here (it clears the mapping), so the key
        // must be present but may be empty.
        let name = require_present_str(args, "name", "set_speaker_name")?;
        Ok(Request::SetSpeakerName {
            id,
            speaker_label,
            name,
        })
    }
}

struct ReassignSpeakerSegment;
impl Tool for ReassignSpeakerSegment {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "reassign_speaker_segment",
            description: "Reassign one transcript segment to a different speaker \
                label. `idx` is the segment's 0-based index (from get_segments); \
                `new_label` is the 1-based [Speaker N] label — a brand-new label \
                simply starts existing.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording whose segment to reassign."
                    },
                    "idx": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "0-based segment index (the get_segments \
                            array order)."
                    },
                    "new_label": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The 1-based [Speaker N] label to assign it to."
                    }
                },
                "required": ["id", "idx", "new_label"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "reassign_speaker_segment")?;
        let idx = require_i64_min(args, "idx", 0, "reassign_speaker_segment")?;
        let new_label = require_i64_min(args, "new_label", 1, "reassign_speaker_segment")?;
        Ok(Request::ReassignSegmentSpeaker {
            id,
            idx,
            new_label,
        })
    }
}

struct MergeSpeakers;
impl Tool for MergeSpeakers {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "merge_speakers",
            description: "Merge two speakers in a recording: every `from_label` \
                segment becomes `into_label`, then `from_label` ceases to exist. \
                `into` keeps its name (adopts `from`'s only when unnamed).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording whose speakers to merge."
                    },
                    "from_label": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The 1-based label that ceases to exist."
                    },
                    "into_label": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The 1-based label that absorbs `from`'s segments."
                    }
                },
                "required": ["id", "from_label", "into_label"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "merge_speakers")?;
        let from_label = require_i64_min(args, "from_label", 1, "merge_speakers")?;
        let into_label = require_i64_min(args, "into_label", 1, "merge_speakers")?;
        Ok(Request::MergeSpeakers {
            id,
            from_label,
            into_label,
        })
    }
}

struct SplitSpeaker;
impl Tool for SplitSpeaker {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "split_speaker",
            description: "Split some of a speaker's segments off onto a fresh \
                label. The listed `segment_idxs` move from `label` to \
                `new_label` (which starts with no name); every other segment of \
                `label` stays.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording whose speaker to split."
                    },
                    "label": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The 1-based source label to split off of."
                    },
                    "segment_idxs": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 0 },
                        "minItems": 1,
                        "description": "The 0-based segment indices to move onto \
                            `new_label`."
                    },
                    "new_label": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The 1-based fresh label to assign the \
                            listed segments."
                    }
                },
                "required": ["id", "label", "segment_idxs", "new_label"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "split_speaker")?;
        let label = require_i64_min(args, "label", 1, "split_speaker")?;
        let new_label = require_i64_min(args, "new_label", 1, "split_speaker")?;
        // A non-empty array of non-negative segment indices.
        let raw = args
            .get("segment_idxs")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::BadArgs {
                tool: "split_speaker".to_string(),
                reason: "missing required array `segment_idxs`".to_string(),
            })?;
        if raw.is_empty() {
            return Err(ToolError::BadArgs {
                tool: "split_speaker".to_string(),
                reason: "`segment_idxs` must list at least one segment".to_string(),
            });
        }
        let mut segment_idxs = Vec::with_capacity(raw.len());
        for v in raw {
            let n = v.as_i64().ok_or_else(|| ToolError::BadArgs {
                tool: "split_speaker".to_string(),
                reason: "`segment_idxs` must contain only integers".to_string(),
            })?;
            if n < 0 {
                return Err(ToolError::BadArgs {
                    tool: "split_speaker".to_string(),
                    reason: "`segment_idxs` entries must be 0 or greater".to_string(),
                });
            }
            segment_idxs.push(n);
        }
        Ok(Request::SplitSpeaker {
            id,
            label,
            segment_idxs,
            new_label,
        })
    }
}

struct RecognizeSpeakers;
impl Tool for RecognizeSpeakers {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "recognize_speakers",
            description: "Run named-speaker recognition for a recording: the \
                still-unnamed diarized speakers whose voiceprints match a known \
                voice. Returns the suggestions (empty when recognition is off or \
                nothing matches).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The recording to recognize speakers in."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_recording_id(args, "recognize_speakers")?;
        Ok(Request::RecognizeSpeakers { id })
    }
}

struct ListNamedVoices;
impl Tool for ListNamedVoices {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_named_voices",
            description: "List the named-voice library — id, name, and sample \
                count per enrolled voice.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, _args: &Value) -> Result<Request, ToolError> {
        Ok(Request::ListNamedVoices)
    }
}

struct RenameNamedVoice;
impl Tool for RenameNamedVoice {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rename_named_voice",
            description: "Rename an enrolled named voice in the speaker library.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The named-voice id (from list_named_voices)."
                    },
                    "name": {
                        "type": "string",
                        "description": "The new display name."
                    }
                },
                "required": ["id", "name"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_str(args, "id", "rename_named_voice")?;
        let name = require_str(args, "name", "rename_named_voice")?;
        Ok(Request::RenameNamedVoice { id, name })
    }
}

struct MergeNamedVoices;
impl Tool for MergeNamedVoices {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "merge_named_voices",
            description: "Merge one named voice into another — re-points the \
                source's samples onto the target and deletes the source.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from_id": {
                        "type": "string",
                        "description": "The voice to merge FROM (removed on success)."
                    },
                    "into_id": {
                        "type": "string",
                        "description": "The voice to merge INTO (kept)."
                    }
                },
                "required": ["from_id", "into_id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let from_id = require_str(args, "from_id", "merge_named_voices")?;
        let into_id = require_str(args, "into_id", "merge_named_voices")?;
        Ok(Request::MergeNamedVoices { from_id, into_id })
    }
}

struct ForgetNamedVoice;
impl Tool for ForgetNamedVoice {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "forget_named_voice",
            description: "Forget a named voice — REVERSIBLY: it vanishes from the \
                library and recognition and its captures are unlinked, but the \
                raw per-recording voiceprints stay (an undo path exists in the \
                app). Confirm with the user before calling.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The named-voice id to forget."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let id = require_str(args, "id", "forget_named_voice")?;
        Ok(Request::ForgetNamedVoice { id })
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
                "get_segments",
                "approve_tag_suggestion",
                "dismiss_tag_suggestion",
                "start_meeting",
                "stop_meeting",
                "list_meeting",
                "set_speaker_name",
                "reassign_speaker_segment",
                "merge_speakers",
                "split_speaker",
                "recognize_speakers",
                "list_named_voices",
                "rename_named_voice",
                "merge_named_voices",
                "forget_named_voice",
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
                limit: 10,
                filter: None,
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

    // ── S5 batch: segments, tag-suggestion approve/dismiss ───────────────

    #[test]
    fn get_segments_maps_and_validates_id() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request("get_segments", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::GetSegments { id }
        );
        assert!(matches!(
            r.to_request("get_segments", &json!({ "id": "nope" })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn approve_and_dismiss_tag_suggestion_require_name() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request(
                "approve_tag_suggestion",
                &json!({ "id": id.as_str(), "name": "work" })
            )
            .unwrap(),
            Request::ApproveTagSuggestion {
                id: id.clone(),
                name: "work".to_string()
            }
        );
        assert_eq!(
            r.to_request(
                "dismiss_tag_suggestion",
                &json!({ "id": id.as_str(), "name": "spam" })
            )
            .unwrap(),
            Request::DismissTagSuggestion {
                id: id.clone(),
                name: "spam".to_string()
            }
        );
        // Missing / blank name → BadArgs (never reaches the daemon).
        assert!(matches!(
            r.to_request("approve_tag_suggestion", &json!({ "id": id.as_str() })),
            Err(ToolError::BadArgs { .. })
        ));
        assert!(matches!(
            r.to_request(
                "dismiss_tag_suggestion",
                &json!({ "id": id.as_str(), "name": "  " })
            ),
            Err(ToolError::BadArgs { .. })
        ));
    }

    // ── S5 batch: meetings ────────────────────────────────────────────────

    #[test]
    fn meeting_start_stop_are_unit() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("start_meeting", &json!({})).unwrap(),
            Request::StartMeeting
        );
        assert_eq!(
            r.to_request("stop_meeting", &json!({})).unwrap(),
            Request::StopMeeting
        );
    }

    #[test]
    fn list_meeting_requires_meeting_id() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("list_meeting", &json!({ "meeting_id": "m-123" }))
                .unwrap(),
            Request::ListMeeting {
                meeting_id: "m-123".to_string()
            }
        );
        assert!(matches!(
            r.to_request("list_meeting", &json!({})),
            Err(ToolError::BadArgs { .. })
        ));
    }

    // ── S5 batch: speaker correction + recognition ────────────────────────

    #[test]
    fn set_speaker_name_allows_blank_to_clear() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        // A real name.
        assert_eq!(
            r.to_request(
                "set_speaker_name",
                &json!({ "id": id.as_str(), "speaker_label": 2, "name": "Alex" })
            )
            .unwrap(),
            Request::SetSpeakerName {
                id: id.clone(),
                speaker_label: 2,
                name: "Alex".to_string()
            }
        );
        // A blank name is meaningful — it clears the mapping; the request still
        // builds (with an empty trimmed name).
        assert_eq!(
            r.to_request(
                "set_speaker_name",
                &json!({ "id": id.as_str(), "speaker_label": 1, "name": "  " })
            )
            .unwrap(),
            Request::SetSpeakerName {
                id: id.clone(),
                speaker_label: 1,
                name: String::new()
            }
        );
        // A label below 1 → BadArgs.
        assert!(matches!(
            r.to_request(
                "set_speaker_name",
                &json!({ "id": id.as_str(), "speaker_label": 0, "name": "x" })
            ),
            Err(ToolError::BadArgs { .. })
        ));
        // A missing name key → BadArgs (the key must be present even when blank).
        assert!(matches!(
            r.to_request(
                "set_speaker_name",
                &json!({ "id": id.as_str(), "speaker_label": 1 })
            ),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn reassign_speaker_segment_maps_and_validates_bounds() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request(
                "reassign_speaker_segment",
                &json!({ "id": id.as_str(), "idx": 0, "new_label": 3 })
            )
            .unwrap(),
            Request::ReassignSegmentSpeaker {
                id: id.clone(),
                idx: 0,
                new_label: 3
            }
        );
        // idx must be 0 or greater; label 1 or greater.
        assert!(matches!(
            r.to_request(
                "reassign_speaker_segment",
                &json!({ "id": id.as_str(), "idx": -1, "new_label": 1 })
            ),
            Err(ToolError::BadArgs { .. })
        ));
        assert!(matches!(
            r.to_request(
                "reassign_speaker_segment",
                &json!({ "id": id.as_str(), "idx": 0, "new_label": 0 })
            ),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn merge_speakers_maps_both_labels() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request(
                "merge_speakers",
                &json!({ "id": id.as_str(), "from_label": 2, "into_label": 1 })
            )
            .unwrap(),
            Request::MergeSpeakers {
                id: id.clone(),
                from_label: 2,
                into_label: 1
            }
        );
        assert!(matches!(
            r.to_request(
                "merge_speakers",
                &json!({ "id": id.as_str(), "from_label": 0, "into_label": 1 })
            ),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn split_speaker_maps_and_validates_idx_list() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request(
                "split_speaker",
                &json!({ "id": id.as_str(), "label": 1, "segment_idxs": [0, 2, 5], "new_label": 2 })
            )
            .unwrap(),
            Request::SplitSpeaker {
                id: id.clone(),
                label: 1,
                segment_idxs: vec![0, 2, 5],
                new_label: 2
            }
        );
        // Empty list → BadArgs.
        assert!(matches!(
            r.to_request(
                "split_speaker",
                &json!({ "id": id.as_str(), "label": 1, "segment_idxs": [], "new_label": 2 })
            ),
            Err(ToolError::BadArgs { .. })
        ));
        // Negative / non-integer entry → BadArgs.
        assert!(matches!(
            r.to_request(
                "split_speaker",
                &json!({ "id": id.as_str(), "label": 1, "segment_idxs": [-1], "new_label": 2 })
            ),
            Err(ToolError::BadArgs { .. })
        ));
        assert!(matches!(
            r.to_request(
                "split_speaker",
                &json!({ "id": id.as_str(), "label": 1, "segment_idxs": ["a"], "new_label": 2 })
            ),
            Err(ToolError::BadArgs { .. })
        ));
        // Missing list → BadArgs.
        assert!(matches!(
            r.to_request(
                "split_speaker",
                &json!({ "id": id.as_str(), "label": 1, "new_label": 2 })
            ),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn recognize_speakers_maps_and_validates_id() {
        let r = ToolRegistry::with_phoneme_tools();
        let id = RecordingId::new();
        assert_eq!(
            r.to_request("recognize_speakers", &json!({ "id": id.as_str() }))
                .unwrap(),
            Request::RecognizeSpeakers { id }
        );
        assert!(matches!(
            r.to_request("recognize_speakers", &json!({ "id": "nope" })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    // ── S5 batch: named-voice library ─────────────────────────────────────

    #[test]
    fn list_named_voices_is_unit() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("list_named_voices", &json!({})).unwrap(),
            Request::ListNamedVoices
        );
    }

    #[test]
    fn rename_named_voice_requires_id_and_name() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request(
                "rename_named_voice",
                &json!({ "id": "v1", "name": "Sam" })
            )
            .unwrap(),
            Request::RenameNamedVoice {
                id: "v1".to_string(),
                name: "Sam".to_string()
            }
        );
        assert!(matches!(
            r.to_request("rename_named_voice", &json!({ "id": "v1" })),
            Err(ToolError::BadArgs { .. })
        ));
        assert!(matches!(
            r.to_request("rename_named_voice", &json!({ "name": "Sam" })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn merge_named_voices_requires_both_ids() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request(
                "merge_named_voices",
                &json!({ "from_id": "a", "into_id": "b" })
            )
            .unwrap(),
            Request::MergeNamedVoices {
                from_id: "a".to_string(),
                into_id: "b".to_string()
            }
        );
        assert!(matches!(
            r.to_request("merge_named_voices", &json!({ "from_id": "a" })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn forget_named_voice_requires_id() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("forget_named_voice", &json!({ "id": "v9" }))
                .unwrap(),
            Request::ForgetNamedVoice {
                id: "v9".to_string()
            }
        );
        assert!(matches!(
            r.to_request("forget_named_voice", &json!({})),
            Err(ToolError::BadArgs { .. })
        ));
    }
}
