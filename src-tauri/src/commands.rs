//! Tauri commands — frontend invokes these via `invoke("…")`.

use crate::bridge::Bridge;
use crate::config_io;
use crate::doctor::CheckResult;
use crate::wizard::TestConnectResult;
use futures::StreamExt;
use phoneme_core::{Config, ListFilter, RecordMode, RecordingId};
use phoneme_ipc::{Request, Response};
use serde_json::Value;
use tauri::{Emitter, State};

type Br<'r> = State<'r, Option<Bridge>>;

/// Structured error returned by Tauri commands. Serializes to `{ kind, message }`
/// so the WebView can branch on `kind` (e.g. tell `whisper_timeout` apart from
/// `not_found`) instead of parsing a flattened `"kind: message"` string. (A-H6)
///
/// `From<String>`/`From<&str>` map ad-hoc errors (config IO, validation) to a
/// generic `"error"` kind, so a command body's `?` on a `Result<_, String>`
/// helper still converts cleanly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandError {
    pub kind: String,
    pub message: String,
}

impl CommandError {
    fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self {
            kind: "error".into(),
            message,
        }
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self {
            kind: "error".into(),
            message: message.into(),
        }
    }
}

async fn forward(bridge: &Option<Bridge>, req: Request) -> Result<Value, CommandError> {
    let bridge = bridge.as_ref().ok_or_else(|| {
        CommandError::new(
            "daemon_not_running",
            "daemon not reachable; start it with `phoneme daemon --start`",
        )
    })?;
    match bridge.request(req).await {
        Ok(Response::Ok(v)) => Ok(v),
        Ok(Response::Err(e)) => Err(CommandError::new(json_kind(&e.kind), e.message)),
        Err(e) => Err(CommandError::new(
            "transport",
            format!("transport error: {e}"),
        )),
    }
}

/// Validate a frontend-supplied recording id. A malformed id reaching the
/// daemon would risk a panic in `RecordingId`'s fixed-offset slicing
/// accessors; reject it here with a clean error instead.
fn parse_id(id: &str) -> Result<RecordingId, CommandError> {
    RecordingId::parse(id)
        .ok_or_else(|| CommandError::new("invalid_config", format!("invalid recording id: {id:?}")))
}

fn json_kind(k: &phoneme_ipc::IpcErrorKind) -> &'static str {
    use phoneme_ipc::IpcErrorKind::*;
    match k {
        AlreadyRecording => "already_recording",
        NotRecording => "not_recording",
        NotFound => "not_found",
        InvalidConfig => "invalid_config",
        WhisperUnreachable => "whisper_unreachable",
        WhisperTimeout => "whisper_timeout",
        HookFailed => "hook_failed",
        DaemonNotRunning => "daemon_not_running",
        PipeInUse => "pipe_in_use",
        ShuttingDown => "shutting_down",
        Io => "io",
        Internal => "internal",
    }
}

/// Fetch a filtered list of all audio recordings.
/// Forwards a `ListRecordings` request to the background daemon.
#[tauri::command]
pub async fn list_recordings(
    bridge: Br<'_>,
    filter: Option<ListFilter>,
) -> Result<Value, CommandError> {
    let filter = filter.unwrap_or_default();
    forward(&bridge, Request::ListRecordings { filter }).await
}

/// Perform a semantic search across transcripts.
#[tauri::command]
pub async fn semantic_search(
    bridge: Br<'_>,
    query: String,
    limit: usize,
) -> Result<Value, CommandError> {
    forward(&bridge, Request::SemanticSearch { query, limit }).await
}

/// Clear all embeddings and re-embed the whole library with the current model
/// (run after changing the embedding model). Returns immediately; runs in the
/// background on the daemon.
#[tauri::command]
pub async fn reembed_all(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ReembedAll).await
}

/// Fetch the details, tags, and transcript for a specific recording by its ID.
#[tauri::command]
pub async fn get_recording(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetRecording { id }).await
}

/// Fetch all recordings belonging to a single meeting session (the two tracks
/// linked by a shared `meeting_id`), ordered by track then start time. Used by
/// the recordings list to render a meeting as one collapsible group.
#[tauri::command]
pub async fn list_meeting(bridge: Br<'_>, meeting_id: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListMeeting { meeting_id }).await
}

/// Fetch one recording's machine transcript segments in timeline order
/// (start/end ms into the track's audio, text, optional speaker label). An
/// empty list is normal — older recordings predate segment capture and some
/// providers return no timing data. Powers the timeline views.
#[tauri::command]
pub async fn get_segments(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetSegments { id }).await
}

/// Delete a recording from the catalog.
/// If `keep_audio` is false, the `.wav` file on disk will also be permanently deleted.
#[tauri::command]
pub async fn delete_recording(
    bridge: Br<'_>,
    id: String,
    keep_audio: bool,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::DeleteRecording { id, keep_audio }).await
}

/// Signal the daemon to start recording audio from the active input device.
/// The `mode` dictates whether this is a continuous push-to-talk (`hold`), a `oneshot`,
/// or a fixed duration recording (`duration:X`).
#[tauri::command]
pub async fn record_start(bridge: Br<'_>, mode: String) -> Result<Value, CommandError> {
    let mode = match mode.as_str() {
        "hold" => RecordMode::Hold,
        "oneshot" => RecordMode::Oneshot,
        other => {
            if let Some(secs) = other.strip_prefix("duration:") {
                let secs: u32 = secs.parse().map_err(|_| "bad duration")?;
                RecordMode::Duration { secs }
            } else {
                return Err(format!("unknown mode: {other}").into());
            }
        }
    };
    forward(
        &bridge,
        Request::RecordStart {
            mode,
            in_place: false,
        },
    )
    .await
}

/// Signal the daemon to cleanly stop the current recording and begin transcription.
#[tauri::command]
pub async fn record_stop(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RecordStop).await
}

/// Signal the daemon to immediately abort the current recording and discard the audio buffer.
#[tauri::command]
pub async fn record_cancel(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RecordCancel).await
}

/// Meeting Mode (v1.6): start a dual-track recording. The daemon captures the
/// microphone AND the system audio (WASAPI loopback) concurrently as two
/// separate recordings linked by a shared `meeting_id`. Returns `{ meeting_id }`.
#[tauri::command]
pub async fn start_meeting(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::StartMeeting).await
}

/// Stop the active meeting. Both tracks are finalized and transcribed.
#[tauri::command]
pub async fn stop_meeting(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::StopMeeting).await
}

/// Signal the daemon to pause the current recording. Audio captured while
/// paused is discarded; recording continues into the same file on resume.
#[tauri::command]
pub async fn record_pause(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RecordPause).await
}

/// Signal the daemon to resume a previously paused recording.
#[tauri::command]
pub async fn record_resume(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RecordResume).await
}

#[tauri::command]
pub async fn retranscribe_recording(
    bridge: Br<'_>,
    id: String,
    model: Option<String>,
    run_hooks: Option<bool>,
    post_process: Option<bool>,
    all_overrides: Option<phoneme_ipc::RerunAllOverrides>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(
        &bridge,
        Request::RetranscribeRecording {
            id,
            model,
            run_hooks,
            post_process,
            all_overrides,
        },
    )
    .await
}

/// Import an existing audio file (wav/mp3/m4a) as a new recording. The daemon
/// decodes it to a canonical WAV and runs it through the normal transcription
/// pipeline. Returns `{ id }` for the new recording.
#[tauri::command]
pub async fn import_recording(bridge: Br<'_>, path: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::ImportRecording { path }).await
}

/// Force the daemon to re-execute the post-processing hook for a given recording ID.
#[tauri::command]
pub async fn refire_hook(
    bridge: Br<'_>,
    id: String,
    command: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::RefireHook { id, command }).await
}

/// Re-run only the LLM post-processing ("cleanup") step on a recording's stored
/// transcript, without re-transcribing the audio. `model` optionally overrides
/// the configured cleanup model for this one run.
#[tauri::command]
pub async fn rerun_cleanup(
    bridge: Br<'_>,
    id: String,
    model: Option<String>,
    provider: Option<String>,
    prompt: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    // A masked key means "use the configured cleanup key" — resolve it here so
    // the real secret is never round-tripped through the WebView.
    let api_key = if api_key.as_deref() == Some(MASKED_SECRET) {
        config_io::read()
            .ok()
            .map(|c| c.llm_post_process.api_key_str().to_owned())
    } else {
        api_key
    };
    forward(
        &bridge,
        Request::RerunCleanup {
            id,
            model,
            provider,
            prompt,
            api_url,
            api_key,
        },
    )
    .await
}

/// Generate (or regenerate) an LLM summary of a recording's current transcript
/// on demand. `model`/`prompt` override the configured summary model/prompt for
/// this run only. The summary arrives via the `SummaryUpdated` daemon event.
#[tauri::command]
pub async fn rerun_summary(
    bridge: Br<'_>,
    id: String,
    model: Option<String>,
    prompt: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::RerunSummary { id, model, prompt }).await
}

/// List the transcription pipeline queue (pending + processing items).
#[tauri::command]
pub async fn list_queue(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListQueue).await
}

/// Run all health checks for the GUI Doctor view.
#[tauri::command]
pub async fn run_doctor(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RunDoctor).await
}

/// Remove a still-pending recording from the queue.
#[tauri::command]
pub async fn cancel_queued(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::CancelQueued { id }).await
}

/// Set the pending queue's claim order (full ordered id list).
#[tauri::command]
pub async fn reorder_queue(bridge: Br<'_>, ids: Vec<String>) -> Result<Value, CommandError> {
    let parsed: Result<Vec<_>, _> = ids.iter().map(|s| parse_id(s)).collect();
    forward(&bridge, Request::ReorderQueue { ids: parsed? }).await
}

/// Pause or resume the transcription queue.
#[tauri::command]
pub async fn set_queue_paused(bridge: Br<'_>, paused: bool) -> Result<Value, CommandError> {
    forward(&bridge, Request::SetQueuePaused { paused }).await
}

/// Query whether the transcription queue is currently paused.
#[tauri::command]
pub async fn queue_paused(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::QueuePaused).await
}

/// Return inbox depth counts (pending/processing/done/failed) on demand, so a
/// freshly-loaded UI shows accurate counts without waiting for an event.
#[tauri::command]
pub async fn queue_counts(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::QueueCounts).await
}

/// Clear the inbox `failed/` quarantine ("dismiss failed"). Returns the count.
#[tauri::command]
pub async fn clear_failed(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ClearFailed).await
}

/// Remove ALL still-pending items from the queue ("clear queue").
#[tauri::command]
pub async fn cancel_all_queued(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::CancelAllQueued).await
}

/// Cancel the item currently being processed (abort the in-flight transcription/LLM).
#[tauri::command]
pub async fn cancel_processing(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::CancelProcessing { id }).await
}

/// Manually update the transcript text for a specific recording.
#[tauri::command]
pub async fn update_transcript(
    bridge: Br<'_>,
    id: String,
    text: String,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::UpdateTranscript { id, text }).await
}

#[tauri::command]
pub async fn update_meeting_name(
    bridge: Br<'_>,
    meeting_id: String,
    name: Option<String>,
) -> Result<Value, CommandError> {
    forward(&bridge, Request::UpdateMeetingName { meeting_id, name }).await
}

/// Fetch the preserved original (machine) transcript for a recording, if any.
#[tauri::command]
pub async fn get_original_transcript(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetOriginalTranscript { id }).await
}

/// Fetch the preserved "unedited" transcript (pipeline output before user edits).
#[tauri::command]
pub async fn get_clean_transcript(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetCleanTranscript { id }).await
}

/// Update the free-form user notes for a specific recording. Independent of the
/// transcript; never affected by (re-)transcription.
#[tauri::command]
pub async fn update_notes(
    bridge: Br<'_>,
    id: String,
    notes: String,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::UpdateNotes { id, notes }).await
}

/// Set or clear the "favorite"/star flag for a recording (Favorites view).
#[tauri::command]
pub async fn set_favorite(
    bridge: Br<'_>,
    id: String,
    favorite: bool,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::SetFavorite { id, favorite }).await
}

/// Persist every window's position/size NOW. tauri-plugin-window-state only
/// saves on a graceful exit — a crash, force-kill, or dev-watcher rebuild
/// loses any move/resize since launch. The live-preview overlay calls this
/// (debounced) after the user drags or resizes it, so its placement survives
/// anything.
#[tauri::command]
pub fn save_window_state(app: tauri::AppHandle) -> Result<(), CommandError> {
    use tauri_plugin_window_state::{AppHandleExt, StateFlags};
    // Everything EXCEPT visibility — saving "visible" while the overlay was up
    // (preview/drag) made it restore visible and pop open on every app start.
    app.save_window_state(StateFlags::all() & !StateFlags::VISIBLE)
        .map_err(|e| CommandError::new("internal", e.to_string()))
}

/// Force-restart the bundled whisper-server(s) — the Doctor's "Fix" for an
/// unreachable local Whisper (sweeps hung/orphaned processes; supervisors
/// respawn from the current config).
#[tauri::command]
pub async fn restart_whisper(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RestartWhisper).await
}

/// Switch which meeting track ("mic" / "system") feeds the live preview —
/// the overlay's source toggle (meeting_preview = "toggle").
#[tauri::command]
pub async fn set_preview_source(bridge: Br<'_>, track: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::SetPreviewSource { track }).await
}

/// Skip the pipeline step currently running for the active queue item (the
/// LLM stages — cleanup / summary / tagging). The pipeline continues.
#[tauri::command]
pub async fn skip_current_stage(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::SkipCurrentStage).await
}

/// Run the LLM tag-suggestion step for one recording on demand.
#[tauri::command]
pub async fn suggest_tags(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::SuggestTags { id }).await
}

/// Approve one suggested tag (create if needed + attach + drop the suggestion).
#[tauri::command]
pub async fn approve_tag_suggestion(
    bridge: Br<'_>,
    id: String,
    name: String,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::ApproveTagSuggestion { id, name }).await
}

/// Dismiss one suggested tag (drop it from the recording's suggestion list).
#[tauri::command]
pub async fn dismiss_tag_suggestion(
    bridge: Br<'_>,
    id: String,
    name: String,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::DismissTagSuggestion { id, name }).await
}

/// Set (or clear) the custom display name for one diarized speaker label of a
/// recording. `speaker_label` is the 1-based `[Speaker N]` index; a blank `name`
/// clears the mapping. The stored transcript is never rewritten — names are
/// applied at display/export time. The updated map is reflected on the next
/// `get_recording`/`list_recordings`; a `SpeakerNameUpdated` event also fires.
#[tauri::command]
pub async fn set_speaker_name(
    bridge: Br<'_>,
    id: String,
    speaker_label: i64,
    name: String,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(
        &bridge,
        Request::SetSpeakerName {
            id,
            speaker_label,
            name,
        },
    )
    .await
}

/// Check the background daemon's current runtime status.
/// Returns whether the daemon is actively running and its process ID.
#[tauri::command]
pub async fn daemon_status(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::DaemonStatus).await
}

/// Current capture status: `{ recording: bool, id: Option<String>, meeting: bool }`.
/// Lets the UI re-sync its record/meeting buttons after a reload, since the
/// daemon outlives the app window and a meeting may already be in progress.
#[tauri::command]
pub async fn record_status(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RecordStatus).await
}

/// Placeholder the WebView sees in place of any saved API key, so secrets never
/// leave the daemon/tray process (S-H2). When the WebView writes config back, an
/// unchanged key arrives as this sentinel and we restore the real on-disk value
/// instead of clobbering it. The frontend mirrors this constant.
const MASKED_SECRET: &str = "__phoneme_secret_kept__";

/// Replace every non-empty API key in a serialized config with the mask.
fn mask_config_secrets(v: &mut Value) {
    for section in [
        "whisper",
        "llm_post_process",
        "summary",
        "auto_tag",
        "preview_whisper",
    ] {
        if let Some(key) = v.get_mut(section).and_then(|s| s.get_mut("api_key")) {
            if key.as_str().is_some_and(|k| !k.is_empty()) {
                *key = Value::String(MASKED_SECRET.to_string());
            }
        }
    }
    // The dictation STT lives one level deeper (`in_place.stt.api_key`).
    if let Some(key) = v
        .get_mut("in_place")
        .and_then(|s| s.get_mut("stt"))
        .and_then(|s| s.get_mut("api_key"))
    {
        if key.as_str().is_some_and(|k| !k.is_empty()) {
            *key = Value::String(MASKED_SECRET.to_string());
        }
    }
}

/// Restore any masked key in an incoming config from the current on-disk config,
/// so saving without changing a key keeps it rather than writing the placeholder.
fn unmask_config_secrets(incoming: &mut Config, current: &Config) {
    if incoming.whisper.api_key_str() == MASKED_SECRET {
        incoming
            .whisper
            .set_api_key(current.whisper.api_key_str().to_owned());
    }
    if incoming.llm_post_process.api_key_str() == MASKED_SECRET {
        incoming
            .llm_post_process
            .set_api_key(current.llm_post_process.api_key_str().to_owned());
    }
    if incoming.summary.api_key_str() == MASKED_SECRET {
        incoming
            .summary
            .set_api_key(current.summary.api_key_str().to_owned());
    }
    if incoming.auto_tag.api_key_str() == MASKED_SECRET {
        incoming
            .auto_tag
            .set_api_key(current.auto_tag.api_key_str().to_owned());
    }
    if let Some(pw) = incoming.preview_whisper.as_mut() {
        if pw.api_key_str() == MASKED_SECRET {
            let cur = current
                .preview_whisper
                .as_ref()
                .map(|c| c.api_key_str().to_owned())
                .unwrap_or_default();
            pw.set_api_key(cur);
        }
    }
    if let Some(stt) = incoming.in_place.stt.as_mut() {
        if stt.api_key_str() == MASKED_SECRET {
            let cur = current
                .in_place
                .stt
                .as_ref()
                .map(|c| c.api_key_str().to_owned())
                .unwrap_or_default();
            stt.set_api_key(cur);
        }
    }
}

/// Read the config for the WebView with all API keys masked, so secrets never
/// cross the IPC boundary into the renderer (S-H2). Tray/daemon code that needs
/// the real keys reads `config_io::read()` directly instead.
#[tauri::command]
pub fn read_config() -> Result<Value, CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    let mut json = serde_json::to_value(&cfg).map_err(|e| CommandError::from(e.to_string()))?;
    mask_config_secrets(&mut json);
    Ok(json)
}

/// Show, hide, or move the system-wide live-preview overlay window.
///
/// The overlay normally drives its own visibility from the daemon event stream
/// (see `frontend/src/overlay.ts`), so the frontend rarely needs this — but it
/// exposes explicit control for: a Settings "preview the overlay" button, future
/// keyboard toggles, and re-positioning the card programmatically. The window is
/// created lazily if the setting is on but it hasn't been built yet.
///
/// `action` is one of `"show"`, `"hide"`, or `"move"`. For `"move"`, pass
/// logical `x`/`y` (top-left corner); they are ignored for show/hide.
#[tauri::command]
pub fn set_overlay(
    app: tauri::AppHandle,
    action: String,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(), CommandError> {
    use tauri::{Emitter, Manager};
    // Create the window on demand so "show" works even before the first record.
    crate::overlay::ensure(&app);
    let Some(win) = app.get_webview_window(crate::overlay::OVERLAY_LABEL) else {
        return Err(CommandError::new(
            "internal",
            "overlay window could not be created",
        ));
    };
    let map = |e: tauri::Error| CommandError::new("internal", e.to_string());
    match action.as_str() {
        "show" => {
            win.show().map_err(map)?;
            win.set_always_on_top(true).map_err(map)?;
        }
        "hide" => win.hide().map_err(map)?,
        "preview" => {
            // Show it and ask the overlay webview to render placeholder text and
            // stay pinned open (no auto-hide) so the user can position/resize it
            // without recording. The overlay's ✕ closes it.
            win.show().map_err(map)?;
            win.set_always_on_top(true).map_err(map)?;
            let _ = app.emit(crate::overlay::OVERLAY_PREVIEW_EVENT, ());
        }
        "move" => {
            let (x, y) = (x.unwrap_or(0.0), y.unwrap_or(0.0));
            win.set_position(tauri::LogicalPosition::new(x, y))
                .map_err(map)?;
        }
        other => {
            return Err(CommandError::new(
                "invalid_config",
                format!("unknown overlay action: {other:?}"),
            ))
        }
    }
    Ok(())
}

/// Write a new configuration state to `config.toml`.
///
/// This command also applies several side effects:
/// 1. Updates the Windows Registry Run Key for "Start at login".
/// 2. Reloads the daemon to adopt new settings.
/// 3. Dynamically re-registers global keyboard shortcuts in the frontend window.
#[tauri::command]
pub async fn write_config(
    app: tauri::AppHandle,
    bridge: Br<'_>,
    mut config: Config,
) -> Result<(), CommandError> {
    // The WebView only ever held masked keys; restore any unchanged secret from
    // the current on-disk config so saving doesn't overwrite it with the mask.
    let current = config_io::read().unwrap_or_default();
    unmask_config_secrets(&mut config, &current);
    let cfg = config.clone();
    tokio::task::spawn_blocking(move || config_io::write(&cfg))
        .await
        .map_err(|e| CommandError::from(e.to_string()))?
        .map_err(|e| CommandError::from(e.to_string()))?;

    apply_config(&app, &bridge, &config).await;
    Ok(())
}

/// Register the (enabled) global hotkeys for `config` — record, meeting, and
/// in-place. Shared by app startup and `apply_config` so every code path that
/// (re-)registers hotkeys applies ALL three together; previously the logic was
/// duplicated, risking a path that registered only the main hotkey on a profile
/// switch. Does not unregister first — callers re-applying must `unregister_all`.
pub fn register_hotkeys(app: &tauri::AppHandle, config: &Config) {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    let entries = [
        ("record", config.hotkey.enabled, &config.hotkey.combo),
        (
            "meeting",
            config.meeting_hotkey.enabled,
            &config.meeting_hotkey.combo,
        ),
        (
            "in-place",
            config.in_place_hotkey.enabled,
            &config.in_place_hotkey.combo,
        ),
    ];
    for (label, enabled, combo) in entries {
        if !enabled {
            continue;
        }
        match Shortcut::from_str(combo) {
            Ok(shortcut) => {
                if let Err(e) = app.global_shortcut().register(shortcut) {
                    tracing::warn!("failed to register {label} hotkey: {e}");
                }
            }
            Err(e) => tracing::warn!("invalid {label} hotkey combo {combo:?}: {e}"),
        }
    }
}

/// Apply the side effects of a config that has just been written to
/// `config.toml`: refresh the "start at login" registry key, tell the daemon
/// to reload, and re-register the global hotkey. Shared by `write_config` and
/// `switch_profile` so switching a profile behaves identically to a manual save.
async fn apply_config(app: &tauri::AppHandle, bridge: &Option<Bridge>, config: &Config) {
    // Update start at login registry key dynamically
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let exe_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !exe_path.is_empty() {
            if config.tray.start_at_login {
                if let Err(e) = std::process::Command::new("reg")
                    .args([
                        "add",
                        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                        "/v",
                        "Phoneme",
                        "/t",
                        "REG_SZ",
                        "/d",
                        &format!("\"{}\"", exe_path),
                        "/f",
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                {
                    tracing::warn!("Failed to add registry run key: {e}");
                }
            } else {
                if let Err(e) = std::process::Command::new("reg")
                    .args([
                        "delete",
                        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                        "/v",
                        "Phoneme",
                        "/f",
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                {
                    tracing::warn!("Failed to delete registry run key: {e}");
                }
            }
        }
    }

    // Tell daemon to reload
    if let Err(e) = forward(bridge, Request::ReloadConfig).await {
        tracing::warn!("failed to reload daemon config: {e:?}");
    }

    // Create or tear down the system-wide live-preview overlay window to match
    // the (just-saved) `interface.preview_overlay` setting. Creating it here
    // (hidden) means the next recording can show it instantly; turning the
    // setting off closes the window so no invisible webview lingers.
    crate::overlay::sync(app, config.interface.preview_overlay);

    // Dynamically reload hotkeys in the frontend: drop the old set, then
    // register the new config's hotkeys via the shared helper so all three
    // (record, meeting, in-place) are always re-applied together.
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    if let Err(e) = app.global_shortcut().unregister_all() {
        tracing::warn!("failed to unregister shortcuts: {e}");
    }
    register_hotkeys(app, config);
}

/// List the names of all saved config profiles.
#[tauri::command]
pub fn list_profiles() -> Result<Vec<String>, CommandError> {
    phoneme_core::profiles::list_profiles().map_err(|e| CommandError::from(e.to_string()))
}

/// Snapshot the CURRENT `config.toml` and save it as a profile named `name`.
#[tauri::command]
pub fn save_profile(name: String) -> Result<(), CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    phoneme_core::profiles::save_profile(&name, &cfg).map_err(|e| CommandError::from(e.to_string()))
}

/// Switch the active config to profile `name`: load the profile, write it as
/// `config.toml`, then reload the daemon and re-apply side effects (registry,
/// hotkey) — identical to a manual save.
#[tauri::command]
pub async fn switch_profile(
    app: tauri::AppHandle,
    bridge: Br<'_>,
    name: String,
) -> Result<(), CommandError> {
    let config = tokio::task::spawn_blocking(move || -> Result<Config, CommandError> {
        let cfg = phoneme_core::profiles::load_profile(&name)
            .map_err(|e| CommandError::from(e.to_string()))?;
        config_io::write(&cfg).map_err(|e| CommandError::from(e.to_string()))?;
        Ok(cfg)
    })
    .await
    .map_err(|e| CommandError::from(e.to_string()))??;

    apply_config(&app, &bridge, &config).await;
    Ok(())
}

/// Delete the saved profile named `name`. Does not touch the live config.
#[tauri::command]
pub fn delete_profile(name: String) -> Result<(), CommandError> {
    phoneme_core::profiles::delete_profile(&name).map_err(|e| CommandError::from(e.to_string()))
}

/// List saved profiles with metadata (last-modified time) for the Profile Manager.
#[tauri::command]
pub fn list_profiles_detailed() -> Result<Vec<phoneme_core::profiles::ProfileInfo>, CommandError> {
    phoneme_core::profiles::list_profiles_detailed().map_err(|e| CommandError::from(e.to_string()))
}

/// Rename a saved profile. Fails if the source is missing or the target exists.
#[tauri::command]
pub fn rename_profile(from: String, to: String) -> Result<(), CommandError> {
    phoneme_core::profiles::rename_profile(&from, &to)
        .map_err(|e| CommandError::from(e.to_string()))
}

/// Check if a `config.toml` file already exists on disk.
#[tauri::command]
pub fn config_exists() -> bool {
    config_io::exists()
}

/// Resolve the absolute path to the user's `config.toml` file.
#[tauri::command]
pub fn config_path() -> Result<String, CommandError> {
    config_io::config_path()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| CommandError::from(e.to_string()))
}

/// Execute local system checks for the Doctor utility (e.g. assessing audio devices).
#[tauri::command]
pub fn doctor_local_checks() -> Result<Vec<CheckResult>, CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    Ok(crate::doctor::run_local_checks(&cfg))
}

/// Probe remote backends (Whisper, Ollama) for reachability.
/// Uses 3-second timeouts per endpoint so the Doctor UI stays responsive.
#[tauri::command]
pub async fn doctor_backend_checks() -> Result<Vec<CheckResult>, CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    Ok(crate::doctor::run_backend_checks(&cfg).await)
}

/// Attempt to start the background daemon. Used by the Doctor "Fix" button
/// when the daemon check fails. Follows the same auto-spawn logic as startup.
///
/// Note: if the tray app started without a bridge (daemon was down at launch),
/// the bridge `State` holds `None` and cannot be swapped here — Tauri's managed
/// state is immutable after `.manage()`. In that case `start_daemon` still
/// spawns and waits for readiness; subsequent commands that call `forward()`
/// will reconnect automatically on first use via `Bridge::request`'s retry path.
#[tauri::command]
pub async fn start_daemon(bridge: Br<'_>) -> Result<(), CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    crate::auto_spawn::ensure_running(&cfg)
        .await
        .map_err(|e| CommandError::from(e.to_string()))?;
    // If a bridge connection already existed, force a reconnect so the
    // existing transport is fresh after the daemon restart.
    if let Some(b) = bridge.as_ref() {
        let _ = b.reconnect().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn wizard_test_whisper(url: String) -> Result<TestConnectResult, CommandError> {
    Ok(crate::wizard::test_whisper_endpoint(&url).await)
}

#[tauri::command]
pub async fn wizard_test_hook(
    bridge: Br<'_>,
    custom_command: Option<String>,
) -> Result<TestConnectResult, CommandError> {
    Ok(crate::wizard::test_hook(bridge.as_ref(), custom_command).await)
}

#[tauri::command]
pub fn list_input_devices() -> Result<Vec<String>, CommandError> {
    let devices =
        phoneme_audio::list_input_devices().map_err(|e| CommandError::from(e.to_string()))?;
    Ok(devices.into_iter().map(|d| d.name).collect())
}

#[tauri::command]
pub async fn list_tags(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListTags).await
}

#[tauri::command]
pub async fn add_tag(
    bridge: Br<'_>,
    name: String,
    color: Option<String>,
) -> Result<Value, CommandError> {
    forward(&bridge, Request::AddTag { name, color }).await
}

#[tauri::command]
pub async fn attach_tag(
    bridge: Br<'_>,
    recording_id: String,
    tag_id: i64,
) -> Result<Value, CommandError> {
    let recording_id = parse_id(&recording_id)?;
    forward(
        &bridge,
        Request::AttachTag {
            recording_id,
            tag_id,
        },
    )
    .await
}

#[tauri::command]
pub async fn detach_tag(
    bridge: Br<'_>,
    recording_id: String,
    tag_id: i64,
) -> Result<Value, CommandError> {
    let recording_id = parse_id(&recording_id)?;
    forward(
        &bridge,
        Request::DetachTag {
            recording_id,
            tag_id,
        },
    )
    .await
}

#[tauri::command]
pub async fn tags_for(bridge: Br<'_>, recording_id: String) -> Result<Value, CommandError> {
    let recording_id = parse_id(&recording_id)?;
    forward(&bridge, Request::TagsFor { recording_id }).await
}

/// Return ALL tags (including orphaned ones with no recordings attached).
/// Used by the Tag Manager settings UI.
#[tauri::command]
pub async fn list_all_tags(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListAllTags).await
}

/// Rename a tag and/or change its color.
#[tauri::command]
pub async fn update_tag(
    bridge: Br<'_>,
    id: i64,
    name: String,
    color: Option<String>,
) -> Result<Value, CommandError> {
    forward(&bridge, Request::UpdateTag { id, name, color }).await
}

/// Delete a tag by ID and detach it from all recordings.
#[tauri::command]
pub async fn delete_tag(bridge: Br<'_>, id: i64) -> Result<Value, CommandError> {
    forward(&bridge, Request::DeleteTag { id }).await
}

/// Map of tag id → number of recordings it's attached to. Powers the Tag
/// Manager usage counts.
#[tauri::command]
pub async fn tag_usage_counts(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::TagUsageCounts).await
}

/// Merge one tag into another: re-point all of `from_id`'s recordings onto
/// `into_id`, then delete `from_id`.
#[tauri::command]
pub async fn merge_tags(bridge: Br<'_>, from_id: i64, into_id: i64) -> Result<Value, CommandError> {
    forward(&bridge, Request::MergeTags { from_id, into_id }).await
}

#[derive(serde::Serialize, Clone)]
struct DownloadProgress {
    downloaded: u64,
    total: Option<u64>,
}

#[tauri::command]
pub async fn wizard_download_model(
    window: tauri::Window,
    url: String,
    filename: String,
) -> Result<String, CommandError> {
    if filename.contains('/') || filename.contains('\\') || filename.is_empty() {
        return Err(CommandError::from("Invalid filename"));
    }

    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let models_dir = dirs.data_local_dir().join("models");
    tokio::fs::create_dir_all(&models_dir)
        .await
        .map_err(|e| format!("failed to create models dir: {}", e))?;

    let dest_path = models_dir.join(&filename);
    if tokio::fs::metadata(&dest_path).await.is_ok() {
        // Emit a fake progress event so the UI knows it's 100%
        let _ = window.emit(
            "download_progress",
            DownloadProgress {
                downloaded: 1,
                total: Some(1),
            },
        );
        return Ok(dest_path.to_string_lossy().into_owned());
    }

    let mut file = tokio::fs::File::create(&dest_path)
        .await
        .map_err(|e| format!("failed to create file: {}", e))?;

    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("download failed with status: {}", response.status()).into());
    }

    let total = response.content_length();
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("stream error: {}", e).into());
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(format!("write error: {}", e).into());
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit("download_progress", DownloadProgress { downloaded, total });
    }

    Ok(dest_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn wizard_download_semantic_model(window: tauri::Window) -> Result<String, CommandError> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let semantic_dir = dirs.data_local_dir().join("models").join("semantic");
    tokio::fs::create_dir_all(&semantic_dir)
        .await
        .map_err(|e| format!("failed to create semantic model dir: {}", e))?;

    let files = [
        (
            "model.onnx",
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
        ),
        (
            "tokenizer.json",
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
        ),
    ];

    for (filename, url) in files {
        let dest_path = semantic_dir.join(filename);
        if tokio::fs::metadata(&dest_path).await.is_ok() {
            // Already downloaded this file
            let _ = window.emit(
                "semantic_download_progress",
                DownloadProgress {
                    downloaded: 1,
                    total: Some(1),
                },
            );
            continue;
        }

        let mut file = tokio::fs::File::create(&dest_path)
            .await
            .map_err(|e| format!("failed to create file: {}", e))?;

        let response = reqwest::get(url)
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("download failed with status: {}", response.status()).into());
        }

        let total = response.content_length();
        let mut downloaded: u64 = 0;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&dest_path).await;
                    return Err(format!("stream error: {}", e).into());
                }
            };
            if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("write error: {}", e).into());
            }
            downloaded += chunk.len() as u64;

            let _ = window.emit(
                "semantic_download_progress",
                DownloadProgress { downloaded, total },
            );
        }
    }

    Ok(semantic_dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn wizard_download_diarization_model(window: tauri::Window) -> Result<(), CommandError> {
    // Diarization uses speakrs which downloads models automatically via hf-hub
    // Since hf-hub blocks, we run it in a blocking task.
    // The UI handles this as an indeterminate progress bar (total = null).

    let _ = window.emit(
        "diarization_download_progress",
        DownloadProgress {
            downloaded: 0,
            total: None,
        },
    );

    tokio::task::spawn_blocking(move || {
        // Just instantiating the pipeline triggers the download of the 500MB ONNX models to the hf cache
        let _pipeline =
            speakrs::OwnedDiarizationPipeline::from_pretrained(speakrs::ExecutionMode::Cpu)
                .map_err(|e| format!("failed to download diarization models: {}", e))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    // Emit 100% completion so the wizard knows it's done
    let _ = window.emit(
        "diarization_download_progress",
        DownloadProgress {
            downloaded: 1,
            total: Some(1),
        },
    );

    Ok(())
}

#[derive(serde::Serialize)]
pub struct SystemInfo {
    pub ram_mb: u64,
    pub vram_mb: u64,
}

#[tauri::command]
pub fn wizard_get_system_info() -> SystemInfo {
    let mut sys = sysinfo::System::new_all();
    sys.refresh_memory();
    let ram_mb = sys.total_memory() / 1024 / 1024;

    let mut vram_mb = 0;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-Command", "(Get-CimInstance Win32_VideoController | Measure-Object -Property AdapterRAM -Sum).Sum"])
           .creation_flags(CREATE_NO_WINDOW);

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Ok(bytes) = stdout.trim().parse::<u64>() {
                    vram_mb = bytes / 1024 / 1024;
                }
            }
        }
    }

    SystemInfo { ram_mb, vram_mb }
}

#[tauri::command]
pub async fn wizard_list_downloaded_models() -> Result<Vec<String>, CommandError> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let models_dir = dirs.data_local_dir().join("models");
    let mut downloaded = Vec::new();
    let models = [
        "ggml-tiny.en.bin",
        "ggml-base.en.bin",
        "ggml-small.en.bin",
        "ggml-medium.en.bin",
        "ggml-large-v3.bin",
        "ggml-large-v3-turbo.bin",
    ];
    for model in models {
        let path = models_dir.join(model);
        if tokio::fs::metadata(&path).await.is_ok() {
            downloaded.push(path.to_string_lossy().into_owned());
        }
    }
    Ok(downloaded)
}

/// True iff `child`, once canonicalized, is `root` itself or lives under it.
/// Both paths are canonicalized so `..` traversal and symlinks can't escape the
/// allowed root. Returns `false` if either path can't be canonicalized (e.g.
/// doesn't exist) — fail closed.
fn path_within(child: &std::path::Path, root: &std::path::Path) -> bool {
    match (std::fs::canonicalize(child), std::fs::canonicalize(root)) {
        (Ok(c), Ok(r)) => c.starts_with(&r),
        _ => false,
    }
}

#[tauri::command]
pub fn reveal_file(path: String) -> Result<(), CommandError> {
    // Security: the renderer can pass any string here and we hand it to
    // `explorer /select`. Restrict the target to the configured audio directory
    // (the only thing the UI ever reveals — a recording's WAV or the folder
    // itself) so a compromised WebView can't pop Explorer onto arbitrary paths.
    let cfg = config_io::read().map_err(|e| format!("config error: {e}"))?;
    // Expand %VAR%/~ in the configured audio dir before comparing. The path the
    // UI reveals is an absolute, already-expanded path, so a raw config string
    // like "%USERPROFILE%\\Documents\\phoneme\\audio" would never match and the
    // reveal would fail "path not permitted".
    let audio_dir_raw = cfg
        .expanded()
        .map(|c| c.recording.audio_dir)
        .unwrap_or_else(|_| cfg.recording.audio_dir.clone());
    let audio_dir = std::path::PathBuf::from(&audio_dir_raw);
    let requested = std::path::PathBuf::from(&path);
    if requested != audio_dir && !path_within(&requested, &audio_dir) {
        return Err("path not permitted".into());
    }

    #[cfg(target_os = "windows")]
    {
        let path = path.replace("/", "\\");
        std::process::Command::new("explorer")
            .args(["/select,", &path])
            .spawn()
            .map_err(|e| format!("failed to open explorer: {}", e))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Fallback for macOS/Linux if ever needed
        let _ = path;
    }
    Ok(())
}

#[tauri::command]
pub fn read_file_string(path: String) -> Result<String, CommandError> {
    // Security: this command exists only to load the user's configured external
    // vimrc. Restrict it to exactly that file (canonicalized) so a compromised
    // renderer cannot read arbitrary files like ~/.ssh/id_rsa.
    let cfg = config_io::read().map_err(|e| format!("config error: {e}"))?;
    if cfg.editor.vimrc_path.is_empty() {
        return Err("no external vimrc is configured".into());
    }
    let allowed =
        std::fs::canonicalize(&cfg.editor.vimrc_path).map_err(|e| format!("config error: {e}"))?;
    let requested = std::fs::canonicalize(&path)
        .map_err(|e| CommandError::from(format!("failed to read {}: {}", path, e)))?;
    if requested != allowed {
        return Err("path not permitted".into());
    }
    std::fs::read_to_string(&requested)
        .map_err(|e| CommandError::from(format!("failed to read {}: {}", path, e)))
}

#[tauri::command]
pub async fn wizard_download_server(window: tauri::Window) -> Result<String, CommandError> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let bin_dir = dirs.data_local_dir().join("bin");
    tokio::fs::create_dir_all(&bin_dir)
        .await
        .map_err(|e| format!("failed to create bin dir: {}", e))?;

    let exe_path = bin_dir.join("whisper-server.exe");
    if tokio::fs::metadata(&exe_path).await.is_ok() {
        let _ = window.emit(
            "server_download_progress",
            DownloadProgress {
                downloaded: 1,
                total: Some(1),
            },
        );
        return Ok(exe_path.to_string_lossy().into_owned());
    }

    let url =
        "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.4/whisper-bin-x64.zip";

    // Download into a temp file
    let temp_zip = bin_dir.join("whisper-temp.zip");
    let mut file = tokio::fs::File::create(&temp_zip)
        .await
        .map_err(|e| format!("failed to create temp zip file: {}", e))?;

    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("download failed with status: {}", response.status()).into());
    }

    let total = response.content_length();
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&temp_zip).await;
                return Err(format!("stream error: {}", e).into());
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&temp_zip).await;
            return Err(format!("write error: {}", e).into());
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit(
            "server_download_progress",
            DownloadProgress { downloaded, total },
        );
    }

    // Explicitly sync and drop to ensure file is completely written before unzip
    if let Err(e) = file.sync_all().await {
        let _ = tokio::fs::remove_file(&temp_zip).await;
        return Err(format!("failed to flush zip file: {}", e).into());
    }
    drop(file);

    let zip_path = temp_zip.clone();
    let bin_path = bin_dir.clone();

    tokio::task::spawn_blocking(move || -> Result<(), CommandError> {
        let zip_file = std::fs::File::open(&zip_path)
            .map_err(|e| format!("failed to open downloaded zip: {}", e))?;

        let mut archive = zip::ZipArchive::new(zip_file)
            .map_err(|e| format!("failed to read zip archive: {}", e))?;

        for i in 0..archive.len() {
            let mut file = match archive.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let outpath = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };

            if file.is_file() {
                if let Some(file_name) = outpath.file_name().and_then(|n| n.to_str()) {
                    if file_name.ends_with(".exe") || file_name.ends_with(".dll") {
                        let extract_to = bin_path.join(file_name);
                        let mut outfile = std::fs::File::create(&extract_to).map_err(|e| {
                            format!("failed to create output file {}: {}", file_name, e)
                        })?;
                        std::io::copy(&mut file, &mut outfile)
                            .map_err(|e| format!("failed to extract {}: {}", file_name, e))?;
                    }
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    let _ = tokio::fs::remove_file(&temp_zip).await;

    Ok(exe_path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── forward() with no bridge ───────────────────────────────────────────

    #[tokio::test]
    async fn forward_none_bridge_returns_descriptive_error() {
        let result = forward(&None, Request::DaemonStatus).await;
        let err = result.unwrap_err();
        assert_eq!(err.kind, "daemon_not_running");
        assert!(
            err.message.contains("daemon not reachable"),
            "expected daemon-not-reachable message, got: {err:?}"
        );
    }

    // ── config secret masking (S-H2) ──────────────────────────────────────

    #[test]
    fn mask_replaces_only_nonempty_keys() {
        let mut cfg = Config::default();
        cfg.llm_post_process.set_api_key("sk-secret-123");
        let mut json = serde_json::to_value(&cfg).unwrap();
        mask_config_secrets(&mut json);
        assert_eq!(json["llm_post_process"]["api_key"], MASKED_SECRET);
        // Whisper has no key by default — an empty key stays empty (not masked).
        assert_eq!(json["whisper"]["api_key"], "");
    }

    #[test]
    fn unmask_restores_unchanged_key_and_keeps_a_changed_one() {
        let mut current = Config::default();
        current.llm_post_process.set_api_key("real-cleanup-key");
        current.summary.set_api_key("real-summary-key");

        let mut incoming = current.clone();
        // Unchanged field arrives masked → restore from disk.
        incoming.llm_post_process.set_api_key(MASKED_SECRET);
        // Changed field carries the new key → keep it.
        incoming.summary.set_api_key("new-summary-key");

        unmask_config_secrets(&mut incoming, &current);
        assert_eq!(incoming.llm_post_process.api_key_str(), "real-cleanup-key");
        assert_eq!(incoming.summary.api_key_str(), "new-summary-key");
    }

    // ── parse_id ──────────────────────────────────────────────────────────

    #[test]
    fn parse_id_accepts_valid_id() {
        assert!(parse_id("20260519T143500042").is_ok());
    }

    #[test]
    fn parse_id_rejects_garbage() {
        let err = parse_id("not-an-id").unwrap_err();
        assert!(err.message.contains("invalid recording id"));
    }

    #[test]
    fn parse_id_rejects_empty_string() {
        assert!(parse_id("").is_err());
    }

    // ── json_kind exhaustive ──────────────────────────────────────────────

    #[test]
    fn json_kind_covers_all_variants() {
        use phoneme_ipc::IpcErrorKind::*;
        // Ensure every variant maps to a non-empty kebab-case string.
        let all = [
            AlreadyRecording,
            NotRecording,
            NotFound,
            InvalidConfig,
            WhisperUnreachable,
            WhisperTimeout,
            HookFailed,
            DaemonNotRunning,
            PipeInUse,
            ShuttingDown,
            Io,
            Internal,
        ];
        for variant in &all {
            let s = json_kind(variant);
            assert!(!s.is_empty(), "json_kind returned empty for {variant:?}");
            assert!(
                s.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "json_kind should be snake_case, got {s:?}"
            );
        }
    }
}

#[tauri::command]
pub async fn wizard_ping_ollama() -> Result<bool, CommandError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| CommandError::from(e.to_string()))?;
    match client
        .get("http://127.0.0.1:11434/api/version")
        .send()
        .await
    {
        Ok(r) => Ok(r.status().is_success()),
        Err(_) => Ok(false),
    }
}

#[tauri::command]
pub async fn wizard_detect_deps() -> Result<serde_json::Value, CommandError> {
    let mut has_ollama = false;

    // Check if `ollama` CLI is in PATH
    if let Ok(output) = std::process::Command::new("ollama")
        .arg("--version")
        .output()
    {
        if output.status.success() {
            has_ollama = true;
        }
    }

    // Check default Windows installation paths
    if !has_ollama {
        let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
        if !localappdata.is_empty() {
            let ollama_path = std::path::Path::new(&localappdata)
                .join("Programs")
                .join("Ollama")
                .join("ollama.exe");
            if ollama_path.exists() {
                has_ollama = true;
            }
        }
    }

    if !has_ollama {
        let userprofile = std::env::var("USERPROFILE").unwrap_or_default();
        if !userprofile.is_empty() {
            let ollama_dir = std::path::Path::new(&userprofile).join(".ollama");
            if ollama_dir.exists() {
                has_ollama = true;
            }
        }
    }

    Ok(serde_json::json!({
        "ollama": has_ollama,
    }))
}

#[derive(serde::Serialize, Clone)]
pub struct OllamaPullProgress {
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

#[tauri::command]
pub async fn wizard_pull_ollama_model(
    window: tauri::Window,
    model: String,
) -> Result<(), CommandError> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "name": model });
    let response = client
        .post("http://127.0.0.1:11434/api/pull")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("pull failed with status: {}", response.status()).into());
    }

    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {}", e))?;
        if let Ok(s) = std::str::from_utf8(&chunk) {
            for line in s.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    let status = v["status"].as_str().unwrap_or("").to_string();
                    let completed = v["completed"].as_u64();
                    let total = v["total"].as_u64();
                    let _ = window.emit(
                        "ollama_pull_progress",
                        OllamaPullProgress {
                            status,
                            completed,
                            total,
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

/// Hosts Phoneme may download from. Anything else is rejected so a compromised
/// renderer cannot fetch an arbitrary (e.g. malicious .exe) URL that could then
/// be run via wizard_run_installer.
fn is_allowed_download_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    let host = match reqwest::Url::parse(url) {
        Ok(u) => match u.host_str() {
            Some(h) => h.to_ascii_lowercase(),
            None => return false,
        },
        Err(_) => return false,
    };
    const ALLOWED: &[&str] = &[
        "huggingface.co",
        "github.com",
        "objects.githubusercontent.com",
        "ollama.com",
        "registry.ollama.ai",
    ];
    ALLOWED
        .iter()
        .any(|a| host == *a || host.ends_with(&format!(".{a}")))
}

#[tauri::command]
pub async fn wizard_download_file(
    window: tauri::Window,
    url: String,
    filename: String,
) -> Result<String, CommandError> {
    if filename.contains('/') || filename.contains('\\') || filename.is_empty() {
        return Err(CommandError::from("Invalid filename"));
    }
    if !is_allowed_download_url(&url) {
        return Err(CommandError::from(
            "Download URL is not from an allowed host",
        ));
    }

    let dest_path = std::env::temp_dir().join(&filename);

    let mut file = tokio::fs::File::create(&dest_path)
        .await
        .map_err(|e| format!("failed to create file: {}", e))?;

    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("download failed: {}", response.status()).into());
    }

    let total = response.content_length();
    let mut downloaded: u64 = 0;

    use futures::StreamExt;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("stream error: {}", e).into());
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(format!("write error: {}", e).into());
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit("download_progress", DownloadProgress { downloaded, total });
    }

    Ok(dest_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn wizard_run_installer(path: String) -> Result<(), CommandError> {
    let p = std::path::Path::new(&path);
    if !p.starts_with(std::env::temp_dir()) {
        return Err(CommandError::from(
            "Execution is restricted to the temporary directory",
        ));
    }
    if !p.exists() {
        return Err(CommandError::from("Installer file does not exist"));
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(&path)
            .spawn()
            .map_err(|e| format!("failed to run installer: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_file(path: String) -> Result<(), CommandError> {
    if !std::path::Path::new(&path).exists() {
        return Err(format!("File does not exist: {}", path).into());
    }
    #[cfg(target_os = "windows")]
    {
        // Use explorer.exe directly instead of `cmd /c start`: the latter runs
        // through the shell, so a filename containing `&` or `"` could be parsed
        // as commands. explorer takes the path literally — no shell layer.
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("failed to open file: {}", e))?;
    }
    Ok(())
}

/// Open the user's hooks directory in the file manager, creating it if missing.
///
/// The Doctor "Fix" button previously passed literal `%LOCALAPPDATA%`/`%APPDATA%`
/// strings to `open_file`, which does no env-var expansion — so the path never
/// existed and nothing opened. Resolve the real directory here instead: it lives
/// under the per-user config dir (`config_dir()/hooks`), matching where the
/// daemon's first-run copy writes the reference hooks.
#[tauri::command]
pub fn open_hooks_folder() -> Result<(), CommandError> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| CommandError::from("could not resolve project directories"))?;
    let hooks_dir = dirs.config_dir().join("hooks");
    std::fs::create_dir_all(&hooks_dir)
        .map_err(|e| CommandError::from(format!("failed to create hooks dir: {e}")))?;
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&hooks_dir)
            .spawn()
            .map_err(|e| CommandError::from(format!("failed to open hooks folder: {e}")))?;
    }
    Ok(())
}
