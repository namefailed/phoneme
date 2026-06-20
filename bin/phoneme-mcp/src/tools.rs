//! A thin MCP adapter over the `phoneme-agent-core` tool registry.
//!
//! Per the roadmap this layer is a *translator*, not a brain: each tool's
//! `tools/call` arguments are validated into exactly one [`Request`], the daemon
//! does the work, and the [`phoneme_ipc::Response`] value is rendered back as MCP
//! text content.
//!
//! There is **no second tool catalog here.** `phoneme-agent-core` is the single
//! source of truth for the tool names, schemas, and the arg→`Request` mapping;
//! this module only adapts that registry to the MCP wire shapes:
//!
//! - [`tools_list`] turns the registry's [`ToolSpec`]s into the JSON-RPC
//!   `tools/list` payload (`name` / `description` / `inputSchema`);
//! - [`build_request`] delegates a `tools/call` name+arguments to the registry's
//!   pure [`ToolRegistry::to_request`], translating its error into the MCP
//!   [`ToolError`];
//! - [`render_result`] is the MCP-presentation half (registry-free): it shapes a
//!   successful [`phoneme_ipc::Response`] value into the text an MCP client shows.
//!
//! `build_request` stays pure and daemon-free, so request-building round-trips in
//! unit tests without a live daemon (mirroring how `bin/phoneme`'s command tests
//! assert the exact `Request` a subcommand sends).

use phoneme_agent_core::{ToolRegistry, ToolSpec};
use serde_json::{json, Value};

// Re-export so call sites keep their `Request` reference; the request-building
// lives in the registry now.
use phoneme_ipc::Request;

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

/// The shared registry — the single source of truth for the tool catalog and the
/// arg→`Request` mapping. Cheap to build; rebuilt per call so the surface stays
/// stateless.
fn registry() -> ToolRegistry {
    ToolRegistry::with_phoneme_tools()
}

/// The JSON-RPC `tools/list` payload: every tool with its input schema.
///
/// Built by iterating the registry's [`ToolSpec`]s so the advertised surface can
/// never drift from [`build_request`] — both read the same catalog.
pub fn tools_list() -> Value {
    let tools: Vec<Value> = registry()
        .specs()
        .iter()
        .map(|ToolSpec {
                 name,
                 description,
                 input_schema,
             }| {
            json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// Translate a `tools/call` (name + arguments object) into the single
/// [`Request`] it maps to, by delegating to the shared registry.
///
/// Pure and daemon-free: argument validation lives in `phoneme-agent-core` so
/// tests can assert the exact request without standing up a daemon. `arguments`
/// is the raw MCP `arguments` object (may be `null`/absent → treated as empty).
pub fn build_request(name: &str, arguments: &Value) -> Result<Request, ToolError> {
    registry()
        .to_request(name, arguments)
        .map_err(|e| ToolError::new(e.to_string()))
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
        "get_segments" => render_segments(value),
        // `list_meeting` is an array of recording DTOs, same shape `list_recent`
        // renders.
        "list_meeting" => render_recent(value),
        "start_meeting" => match value.get("meeting_id").and_then(Value::as_str) {
            Some(id) => format!("Meeting started. meeting_id: {id}"),
            None => "Meeting started.".to_string(),
        },
        "stop_meeting" => match value.get("meeting_id").and_then(Value::as_str) {
            Some(id) => format!("Meeting stopped. meeting_id: {id}"),
            None => "Meeting stopped.".to_string(),
        },
        "recognize_speakers" => render_speaker_suggestions(value),
        "list_named_voices" => render_named_voices(value),
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
        "approve_tag_suggestion" => match value.get("name").and_then(Value::as_str) {
            Some(name) => format!("Tag '{name}' approved and attached."),
            None => "Tag suggestion approved.".to_string(),
        },
        "dismiss_tag_suggestion" => "Tag suggestion dismissed.".to_string(),
        // The in-recording speaker edits answer Ok `{}`; a name set answers a
        // propagation block. Either way a short confirmation reads best.
        "set_speaker_name" => "Speaker name updated.".to_string(),
        "reassign_speaker_segment" => "Segment reassigned.".to_string(),
        "merge_speakers" => "Speakers merged.".to_string(),
        "split_speaker" => "Speaker split.".to_string(),
        "rename_named_voice" => "Named voice renamed.".to_string(),
        "merge_named_voices" => "Named voices merged.".to_string(),
        "forget_named_voice" => "Named voice forgotten.".to_string(),
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

/// `get_segments`: an array of segment objects (`{start_ms, end_ms, text,
/// speaker}`) → a count plus a note (the full timings are large; the agent reads
/// them as structured data when it needs them).
fn render_segments(value: &Value) -> String {
    match value.as_array() {
        Some(segs) if !segs.is_empty() => format!(
            "{} transcript segments available (start/end offsets, text, and speaker label per segment).",
            segs.len()
        ),
        Some(_) => "No transcript segments for this recording yet.".to_string(),
        None => pretty(value),
    }
}

/// `recognize_speakers`: an array of `SpeakerSuggestion` (`{speaker_label, name,
/// …}`) → a bulleted "Speaker N → name" list, or a note when nothing matched.
fn render_speaker_suggestions(value: &Value) -> String {
    let Some(hits) = value.as_array() else {
        return pretty(value);
    };
    if hits.is_empty() {
        return "No named-speaker matches.".to_string();
    }
    let mut out = String::new();
    for hit in hits {
        let label = hit.get("speaker_label").and_then(Value::as_i64);
        let name = hit.get("name").and_then(Value::as_str).unwrap_or("(unknown)");
        match label {
            Some(n) => out.push_str(&format!("- Speaker {n} → {name}\n")),
            None => out.push_str(&format!("- {name}\n")),
        }
    }
    out.trim_end().to_string()
}

/// `list_named_voices`: an array of `NamedVoice` (`{id, name, sample_count}`) → a
/// bulleted list of names with their sample counts.
fn render_named_voices(value: &Value) -> String {
    let Some(voices) = value.as_array() else {
        return pretty(value);
    };
    if voices.is_empty() {
        return "No named voices enrolled yet.".to_string();
    }
    let mut out = String::new();
    for v in voices {
        let name = v.get("name").and_then(Value::as_str).unwrap_or("(unnamed)");
        let samples = v.get("sample_count").and_then(Value::as_i64);
        match samples {
            Some(n) => out.push_str(&format!("- {name} ({n} samples)\n")),
            None => out.push_str(&format!("- {name}\n")),
        }
    }
    out.trim_end().to_string()
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
    use phoneme_core::{RecordMode, RecordingId};

    /// Default `limit` the registry applies — kept in step with the catalog.
    const DEFAULT_LIMIT: usize = 10;

    /// Every tool name this bridge exposes, in `tools/list` order. Sourced from
    /// the shared registry, not hand-maintained: the adapter only re-shapes it.
    fn expected_tools() -> Vec<&'static str> {
        registry().specs().iter().map(|s| s.name).collect()
    }

    #[test]
    fn tools_list_has_all_tools_with_schemas() {
        let list = tools_list();
        let tools = list["tools"].as_array().expect("tools array");
        let expected = expected_tools();
        assert_eq!(tools.len(), expected.len());
        for t in tools {
            let name = t["name"].as_str().expect("tool name");
            assert!(expected.contains(&name), "unexpected tool {name}");
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
    fn tools_list_matches_agent_core_registry() {
        // The MCP surface is exactly the agent-core registry, same names, same
        // order — so the two can never drift again.
        let list = tools_list();
        let mcp_names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        let core_names: Vec<&str> = registry().specs().iter().map(|s| s.name).collect();
        assert_eq!(mcp_names, core_names);
    }

    #[test]
    fn tool_names_are_all_dispatchable() {
        let list = tools_list();
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();

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
                whisper_model: None,
                source: None,
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
                whisper_model: None,
                source: None,
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
                limit: 3,
                filter: None,
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
                limit: DEFAULT_LIMIT,
                filter: None,
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

    // ── "act on it" tools ────────────────────────────────────────────────

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
                recipe_id: None,
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
                recipe_id: None,
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
                limit: DEFAULT_LIMIT
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

    // ── S5 batch: build_request mapping ───────────────────────────────────

    #[test]
    fn get_segments_maps_and_rejects_bad_id() {
        let id = RecordingId::new();
        assert_eq!(
            build_request("get_segments", &json!({"id": id.as_str()})).unwrap(),
            Request::GetSegments { id }
        );
        assert!(build_request("get_segments", &json!({"id": "nope"})).is_err());
        assert!(build_request("get_segments", &json!({})).is_err());
    }

    #[test]
    fn tag_suggestion_approve_dismiss_map() {
        let id = RecordingId::new();
        assert_eq!(
            build_request(
                "approve_tag_suggestion",
                &json!({"id": id.as_str(), "name": "work"})
            )
            .unwrap(),
            Request::ApproveTagSuggestion {
                id: id.clone(),
                name: "work".to_string()
            }
        );
        assert_eq!(
            build_request(
                "dismiss_tag_suggestion",
                &json!({"id": id.as_str(), "name": "spam"})
            )
            .unwrap(),
            Request::DismissTagSuggestion {
                id,
                name: "spam".to_string()
            }
        );
        assert!(build_request("approve_tag_suggestion", &json!({"id": "x"})).is_err());
    }

    #[test]
    fn meeting_tools_map() {
        assert_eq!(
            build_request("start_meeting", &json!({})).unwrap(),
            Request::StartMeeting
        );
        assert_eq!(
            build_request("stop_meeting", &json!({})).unwrap(),
            Request::StopMeeting
        );
        assert_eq!(
            build_request("list_meeting", &json!({"meeting_id": "m1"})).unwrap(),
            Request::ListMeeting {
                meeting_id: "m1".to_string()
            }
        );
        assert!(build_request("list_meeting", &json!({})).is_err());
    }

    #[test]
    fn set_speaker_name_maps_and_allows_blank() {
        let id = RecordingId::new();
        assert_eq!(
            build_request(
                "set_speaker_name",
                &json!({"id": id.as_str(), "speaker_label": 2, "name": "Alex"})
            )
            .unwrap(),
            Request::SetSpeakerName {
                id: id.clone(),
                speaker_label: 2,
                name: "Alex".to_string()
            }
        );
        // Blank name clears the mapping (request still builds).
        assert_eq!(
            build_request(
                "set_speaker_name",
                &json!({"id": id.as_str(), "speaker_label": 1, "name": ""})
            )
            .unwrap(),
            Request::SetSpeakerName {
                id,
                speaker_label: 1,
                name: String::new()
            }
        );
        // Missing name key, or a label below 1 → tool error.
        let id2 = RecordingId::new();
        assert!(build_request(
            "set_speaker_name",
            &json!({"id": id2.as_str(), "speaker_label": 1})
        )
        .is_err());
        assert!(build_request(
            "set_speaker_name",
            &json!({"id": id2.as_str(), "speaker_label": 0, "name": "x"})
        )
        .is_err());
    }

    #[test]
    fn speaker_correction_tools_map() {
        let id = RecordingId::new();
        assert_eq!(
            build_request(
                "reassign_speaker_segment",
                &json!({"id": id.as_str(), "idx": 0, "new_label": 3})
            )
            .unwrap(),
            Request::ReassignSegmentSpeaker {
                id: id.clone(),
                idx: 0,
                new_label: 3
            }
        );
        assert_eq!(
            build_request(
                "merge_speakers",
                &json!({"id": id.as_str(), "from_label": 2, "into_label": 1})
            )
            .unwrap(),
            Request::MergeSpeakers {
                id: id.clone(),
                from_label: 2,
                into_label: 1
            }
        );
        assert_eq!(
            build_request(
                "split_speaker",
                &json!({"id": id.as_str(), "label": 1, "segment_idxs": [0, 4], "new_label": 2})
            )
            .unwrap(),
            Request::SplitSpeaker {
                id: id.clone(),
                label: 1,
                segment_idxs: vec![0, 4],
                new_label: 2
            }
        );
        // split_speaker rejects an empty idx list.
        assert!(build_request(
            "split_speaker",
            &json!({"id": id.as_str(), "label": 1, "segment_idxs": [], "new_label": 2})
        )
        .is_err());
        assert_eq!(
            build_request("recognize_speakers", &json!({"id": id.as_str()})).unwrap(),
            Request::RecognizeSpeakers { id }
        );
    }

    #[test]
    fn named_voice_tools_map() {
        assert_eq!(
            build_request("list_named_voices", &json!({})).unwrap(),
            Request::ListNamedVoices
        );
        assert_eq!(
            build_request("rename_named_voice", &json!({"id": "v1", "name": "Sam"})).unwrap(),
            Request::RenameNamedVoice {
                id: "v1".to_string(),
                name: "Sam".to_string()
            }
        );
        assert_eq!(
            build_request(
                "merge_named_voices",
                &json!({"from_id": "a", "into_id": "b"})
            )
            .unwrap(),
            Request::MergeNamedVoices {
                from_id: "a".to_string(),
                into_id: "b".to_string()
            }
        );
        assert_eq!(
            build_request("forget_named_voice", &json!({"id": "v9"})).unwrap(),
            Request::ForgetNamedVoice {
                id: "v9".to_string()
            }
        );
        assert!(build_request("rename_named_voice", &json!({"id": "v1"})).is_err());
        assert!(build_request("merge_named_voices", &json!({"from_id": "a"})).is_err());
        assert!(build_request("forget_named_voice", &json!({})).is_err());
    }

    // ── S5 batch: result rendering ────────────────────────────────────────

    #[test]
    fn render_s5_confirmations_and_lists() {
        assert!(render_result("start_meeting", &json!({"meeting_id": "m1"})).contains("m1"));
        assert!(render_result("stop_meeting", &json!({"meeting_id": "m1"})).contains("m1"));
        assert_eq!(
            render_result("dismiss_tag_suggestion", &Value::Null),
            "Tag suggestion dismissed."
        );
        assert!(render_result("approve_tag_suggestion", &json!({"name": "work"})).contains("work"));
        assert_eq!(
            render_result("set_speaker_name", &json!({"propagation": {"policy": "off"}})),
            "Speaker name updated."
        );
        assert_eq!(
            render_result("merge_speakers", &json!({})),
            "Speakers merged."
        );

        let segs = json!([{"start_ms": 0, "end_ms": 100, "text": "hi", "speaker": "Speaker 1"}]);
        assert!(render_result("get_segments", &segs).contains('1'));
        assert!(render_result("get_segments", &json!([])).contains("No transcript segments"));

        let voices = json!([{"id": "v1", "name": "Sam", "sample_count": 3}]);
        let out = render_result("list_named_voices", &voices);
        assert!(out.contains("Sam"), "got: {out}");
        assert!(out.contains('3'), "got: {out}");
        assert!(render_result("list_named_voices", &json!([])).contains("No named voices"));

        let suggestions = json!([{"speaker_label": 2, "name": "Alex"}]);
        let out = render_result("recognize_speakers", &suggestions);
        assert!(out.contains("Speaker 2"), "got: {out}");
        assert!(out.contains("Alex"), "got: {out}");
        assert!(render_result("recognize_speakers", &json!([])).contains("No named-speaker"));
    }
}
