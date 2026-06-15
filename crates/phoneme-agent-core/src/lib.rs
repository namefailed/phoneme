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
//! The future in-app agent panel drives this registry directly; the standalone
//! opencode-based agent reaches the same capabilities from outside via the
//! `phoneme-mcp` server — "same registry, opposite direction" (see
//! `docs/design/phoneme-agent-harness.md`). The registry intentionally mirrors
//! the five tools `phoneme-mcp` exposes today; richer actions (tag, title,
//! summarize, export, favorite) slot in here as their `Request`s are added.

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

    /// The canonical Phoneme toolset — the same capabilities `phoneme-mcp`
    /// exposes externally (list / search / get / start / stop).
    pub fn with_phoneme_tools() -> Self {
        let mut r = Self::new();
        r.register(Box::new(ListRecent));
        r.register(Box::new(SearchRecordings));
        r.register(Box::new(GetTranscript));
        r.register(Box::new(StartRecording));
        r.register(Box::new(StopRecording));
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

/// Default result cap for the list/search tools (matches `phoneme-mcp`).
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

fn opt_u32(args: &Value, key: &str, default: u32) -> u32 {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(default)
}

struct ListRecent;
impl Tool for ListRecent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_recent",
            description: "List the most recent recordings, newest first.",
            input_schema: json!({
                "type": "object",
                "properties": { "limit": { "type": "integer", "minimum": 1, "description": "Max rows (default 10)." } }
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let limit = opt_u32(args, "limit", DEFAULT_LIMIT);
        Ok(Request::ListRecordings {
            filter: ListFilter { limit: Some(limit), ..Default::default() },
        })
    }
}

struct SearchRecordings;
impl Tool for SearchRecordings {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_recordings",
            description: "Semantic + lexical search over the recording library.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural-language query." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Max hits (default 10)." }
                },
                "required": ["query"]
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let query = require_str(args, "query", "search_recordings")?;
        let limit = opt_u32(args, "limit", DEFAULT_LIMIT) as usize;
        Ok(Request::SemanticSearch { query, limit })
    }
}

struct GetTranscript;
impl Tool for GetTranscript {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_transcript",
            description: "Fetch a recording's transcript by id.",
            input_schema: json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "Recording id from list/search." } },
                "required": ["id"]
            }),
        }
    }
    fn to_request(&self, args: &Value) -> Result<Request, ToolError> {
        let raw = require_str(args, "id", "get_transcript")?;
        let id = RecordingId::parse(raw).ok_or_else(|| ToolError::BadArgs {
            tool: "get_transcript".to_string(),
            reason: "`id` is not a valid recording id".to_string(),
        })?;
        Ok(Request::GetRecording { id })
    }
}

struct StartRecording;
impl Tool for StartRecording {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "start_recording",
            description: "Start a recording. mode: oneshot (auto-stop on silence) or hold (until stop_recording).",
            input_schema: json!({
                "type": "object",
                "properties": { "mode": { "type": "string", "enum": ["oneshot", "hold"], "description": "Default oneshot." } }
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
                    reason: format!("unknown mode `{other}` (use \"oneshot\" or \"hold\")"),
                })
            }
        };
        Ok(Request::RecordStart { mode, in_place: false })
    }
}

struct StopRecording;
impl Tool for StopRecording {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "stop_recording",
            description: "Stop the active recording.",
            input_schema: json!({ "type": "object", "properties": {} }),
        }
    }
    fn to_request(&self, _args: &Value) -> Result<Request, ToolError> {
        Ok(Request::RecordStop)
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
            ["list_recent", "search_recordings", "get_transcript", "start_recording", "stop_recording"]
        );
        // Every spec carries an object schema.
        assert!(r.specs().iter().all(|s| s.input_schema["type"] == "object"));
    }

    #[test]
    fn list_recent_defaults_to_ten_and_honors_limit() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("list_recent", &json!({})).unwrap(),
            Request::ListRecordings { filter: ListFilter { limit: Some(10), ..Default::default() } }
        );
        assert_eq!(
            r.to_request("list_recent", &json!({ "limit": 3 })).unwrap(),
            Request::ListRecordings { filter: ListFilter { limit: Some(3), ..Default::default() } }
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
            r.to_request("search_recordings", &json!({ "query": "api redesign" })).unwrap(),
            Request::SemanticSearch { query: "api redesign".to_string(), limit: 10 }
        );
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
            r.to_request("get_transcript", &json!({ "id": id.as_str() })).unwrap(),
            Request::GetRecording { id }
        );
    }

    #[test]
    fn start_recording_maps_and_validates_mode() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(
            r.to_request("start_recording", &json!({})).unwrap(),
            Request::RecordStart { mode: RecordMode::Oneshot, in_place: false }
        );
        assert_eq!(
            r.to_request("start_recording", &json!({ "mode": "hold" })).unwrap(),
            Request::RecordStart { mode: RecordMode::Hold, in_place: false }
        );
        assert!(matches!(
            r.to_request("start_recording", &json!({ "mode": "weird" })),
            Err(ToolError::BadArgs { .. })
        ));
    }

    #[test]
    fn stop_is_unit_and_unknown_tool_errors() {
        let r = ToolRegistry::with_phoneme_tools();
        assert_eq!(r.to_request("stop_recording", &json!({})).unwrap(), Request::RecordStop);
        assert!(matches!(r.to_request("nope", &json!({})), Err(ToolError::Unknown(_))));
    }
}
