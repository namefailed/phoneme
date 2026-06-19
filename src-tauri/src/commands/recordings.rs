//! (split from the former commands.rs god-file — see mod.rs)

use super::*;

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

/// Recent persisted AI-activity sessions for the 🧠 popout. `recording_id` filters
/// to one recording; omit it for the whole library's recent activity.
#[tauri::command]
pub async fn list_ai_activity(
    bridge: Br<'_>,
    recording_id: Option<String>,
    limit: u32,
) -> Result<Value, CommandError> {
    forward(
        &bridge,
        Request::ListAiActivity {
            recording_id,
            limit,
        },
    )
    .await
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

/// Fetch one recording's machine transcript words in timeline order — the
/// finer per-word layer beneath `get_segments`. Returns a JSON array (possibly
/// empty) of `{ idx, start_ms, end_ms, text, speaker, confidence }`, ordered by
/// `idx`; `confidence` is `null` when the provider gives none. An empty list is
/// normal (older recordings predate word capture, some providers emit no
/// per-word data). Fetched lazily by the word-level features (word seek,
/// confidence highlighting).
#[tauri::command]
pub async fn get_words(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetWords { id }).await
}

/// Drop every pending tag suggestion across the whole library. Returns
/// `{ "cleared": n }`; the daemon's AllTagSuggestionsCleared event refreshes
/// any open views.
#[tauri::command]
pub async fn clear_all_tag_suggestions(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ClearAllTagSuggestions).await
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

/// Delete an entire meeting session — every track sharing `meeting_id` — in one
/// request. If `keep_audio` is false the tracks' `.wav` files are also removed.
#[tauri::command]
pub async fn delete_session(
    bridge: Br<'_>,
    meeting_id: String,
    keep_audio: bool,
) -> Result<Value, CommandError> {
    forward(
        &bridge,
        Request::DeleteSession {
            meeting_id,
            keep_audio,
        },
    )
    .await
}

/// Destructive catalog rebuild from disk: clears every recording row (losing
/// transcripts, edits, tags) and re-imports every WAV as a fresh recording. The
/// daemon does it in-process and refuses while a recording is active. For a
/// corrupt catalog.db, the CLI `phoneme doctor --rebuild-catalog` is the tool.
#[tauri::command]
pub async fn rebuild_catalog(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RebuildCatalog).await
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
            recipe_id: None,
            whisper_model: None,
            source: None,
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
    recipe_id: Option<String>,
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
            recipe_id,
        },
    )
    .await
}

/// Import an existing audio file (wav/mp3/m4a/flac) as a new recording. The daemon
/// decodes it to a canonical WAV and runs it through the normal transcription
/// pipeline. Returns `{ id }` for the new recording.
#[tauri::command]
pub async fn import_recording(bridge: Br<'_>, path: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::ImportRecording { path }).await
}

/// Safe, non-destructive re-import: scan the audio dir and re-link any file with
/// no catalog row (the counterpart to the destructive `doctor --rebuild-catalog`).
/// `dry_run` returns `{ count, paths }` without writing; otherwise `{ count }`.
#[tauri::command]
pub async fn reimport_from_disk(bridge: Br<'_>, dry_run: bool) -> Result<Value, CommandError> {
    forward(&bridge, Request::ReimportFromDisk { dry_run }).await
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

/// Dismiss ONE item from the inbox `failed/` quarantine by id. Returns
/// `{"removed":bool}`.
#[tauri::command]
pub async fn dismiss_failed(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::DismissFailed { id }).await
}

/// All saved searches (user-named library-filter snapshots), newest first.
#[tauri::command]
pub async fn list_saved_searches(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListSavedSearches).await
}

/// Insert or update a saved search by id (the frontend picks the id, owning the
/// by-name upsert / rename-conflict rules).
#[tauri::command]
pub async fn upsert_saved_search(
    bridge: Br<'_>,
    id: String,
    name: String,
    filter_json: String,
) -> Result<Value, CommandError> {
    forward(
        &bridge,
        Request::UpsertSavedSearch {
            id,
            name,
            filter_json,
        },
    )
    .await
}

/// Delete a saved search by id (unknown ids are a no-op).
#[tauri::command]
pub async fn delete_saved_search(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::DeleteSavedSearch { id }).await
}

/// On-demand named-speaker recognition for a recording (#9): the unnamed diarized
/// speakers whose voiceprints match a known voice.
#[tauri::command]
pub async fn recognize_speakers(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::RecognizeSpeakers { id }).await
}

/// Dismiss a recognized-speaker suggestion so it isn't offered again.
#[tauri::command]
pub async fn dismiss_speaker_suggestion(
    bridge: Br<'_>,
    id: String,
    speaker_label: i64,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(
        &bridge,
        Request::DismissSpeakerSuggestion { id, speaker_label },
    )
    .await
}

/// The named-voice library (Speaker Library manager).
#[tauri::command]
pub async fn list_named_voices(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListNamedVoices).await
}

/// Rename a named voice.
#[tauri::command]
pub async fn rename_named_voice(
    bridge: Br<'_>,
    id: String,
    name: String,
) -> Result<Value, CommandError> {
    forward(&bridge, Request::RenameNamedVoice { id, name }).await
}

/// Merge one named voice into another (re-points samples, deletes the source).
#[tauri::command]
pub async fn merge_named_voices(
    bridge: Br<'_>,
    from_id: String,
    into_id: String,
) -> Result<Value, CommandError> {
    forward(&bridge, Request::MergeNamedVoices { from_id, into_id }).await
}

/// Forget a named voice (unlink its captures, delete the entry).
#[tauri::command]
pub async fn forget_named_voice(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::ForgetNamedVoice { id }).await
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

/// Set or clear a recording's display title. `Some` marks the title user-owned
/// (auto generation never overwrites it again); `None` clears it back to auto —
/// it empties now and regenerates on the next pipeline run.
#[tauri::command]
pub async fn set_recording_title(
    bridge: Br<'_>,
    id: String,
    title: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::SetRecordingTitle { id, title }).await
}

/// Render one recording's machine segments as caption text in the requested
/// format. `format` is `"srt"` or `"vtt"` (anything else is rejected as an
/// invalid config). Fetches the segments through the daemon (`GetSegments`),
/// then renders them with `phoneme_core::export`. Returns the caption body as
/// a string for the WebView to drop into a save dialog — no file is written
/// here, so the dialog plugin owns the destination the same way the plain-text
/// transcript export does.
///
/// An empty segment list is the "retranscribe to generate them" case: it comes
/// back as a `not_found` error carrying the same hint the CLI prints, so the
/// caller can toast it instead of saving an empty file.
#[tauri::command]
pub async fn export_captions(
    bridge: Br<'_>,
    id: String,
    format: String,
) -> Result<String, CommandError> {
    let id = parse_id(&id)?;
    // Validate the format up front so a typo never reaches a silent default.
    let fmt = match format.as_str() {
        "srt" => CaptionFormat::Srt,
        "vtt" => CaptionFormat::Vtt,
        other => {
            return Err(CommandError::new(
                "invalid_config",
                format!("unknown caption format {other:?} (expected \"srt\" or \"vtt\")"),
            ));
        }
    };

    let value = forward(&bridge, Request::GetSegments { id }).await?;
    let segments: Vec<TranscriptSegment> = serde_json::from_value(value)
        .map_err(|e| CommandError::new("internal", format!("parsing segments: {e}")))?;

    if segments.is_empty() {
        return Err(CommandError::new(
            "not_found",
            "no segments stored — retranscribe this recording to generate them",
        ));
    }

    Ok(render_captions(&segments, fmt))
}

/// The two caption formats `export_captions` understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptionFormat {
    Srt,
    Vtt,
}

/// Render segments into the chosen caption format. Split out from the command
/// so the format→content mapping is unit-testable without a live bridge.
fn render_captions(segments: &[TranscriptSegment], format: CaptionFormat) -> String {
    match format {
        CaptionFormat::Srt => phoneme_core::export::segments_to_srt(segments),
        CaptionFormat::Vtt => phoneme_core::export::segments_to_vtt(segments),
    }
}

/// Reject export destinations whose extension is an executable/script. The
/// per-recording + library exports are always text / captions / JSON / zip, so an
/// `.exe`/`.bat`/`.ps1`/`.lnk`/… destination can only be an attempt — e.g. from a
/// compromised WebView — to drop an auto-run payload (a Startup-folder script, a
/// sideloaded binary). Defense-in-depth behind the WebView trust boundary; a
/// legitimate export never targets one of these.
fn reject_executable_dest(dest: &str) -> Result<(), CommandError> {
    const BLOCKED: &[&str] = &[
        "exe",
        "bat",
        "cmd",
        "com",
        "scr",
        "pif",
        "ps1",
        "psm1",
        "psd1",
        "vbs",
        "vbe",
        "js",
        "jse",
        "wsf",
        "wsh",
        "msi",
        "msp",
        "msc",
        "lnk",
        "cpl",
        "hta",
        "reg",
        "jar",
        "gadget",
        "sct",
        "shb",
        "dll",
        "sys",
        "application",
    ];
    let ext = std::path::Path::new(dest)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if let Some(ext) = ext {
        if BLOCKED.contains(&ext.as_str()) {
            return Err(CommandError::new(
                "invalid_config",
                format!(
                    "refusing to export to a .{ext} file — exports are text/zip, not executables"
                ),
            ));
        }
    }
    Ok(())
}

/// Write `contents` to `dest` (a path the WebView picked via the save dialog).
///
/// The single write path behind every per-recording export — transcript text,
/// captions, and the full-data JSON. The content is produced in the WebView (or
/// by [`export_captions`] / [`export_recording_json`]) and handed here so the
/// daemon-side bridge process owns the actual file write, exactly like
/// [`export_library_zip`]. That means the WebView never needs the `fs` plugin's
/// write permission for an arbitrary save-dialog path (which `fs:default` denies).
/// The dest is screened by [`reject_executable_dest`] so the write can't be
/// abused to drop an auto-run payload.
#[tauri::command]
pub fn save_text_export(dest: String, contents: String) -> Result<(), CommandError> {
    reject_executable_dest(&dest)?;
    std::fs::write(&dest, contents)
        .map_err(|e| CommandError::new("io", format!("writing {dest}: {e}")))
}

/// Bundle one recording's full data — the catalog row plus its machine
/// segments — into a pretty-printed JSON string for the "Export → All data"
/// action. Returns a string the WebView saves via [`save_text_export`]; segments
/// are best-effort (a recording transcribed before segment capture has none).
#[tauri::command]
pub async fn export_recording_json(bridge: Br<'_>, id: String) -> Result<String, CommandError> {
    let rid = parse_id(&id)?;
    let recording = forward(&bridge, Request::GetRecording { id: rid.clone() }).await?;
    let segments = forward(&bridge, Request::GetSegments { id: rid })
        .await
        .unwrap_or_else(|_| serde_json::json!([]));
    let bundle = serde_json::json!({
        "version": 1,
        "recording": recording,
        "segments": segments,
    });
    serde_json::to_string_pretty(&bundle)
        .map_err(|e| CommandError::new("internal", format!("serializing recording: {e}")))
}

/// Write a portable backup of the whole library to `dest` (a `.zip` path the
/// WebView picked via the save dialog). Mirrors `phoneme export <FILE>`: a
/// `catalog.json` versioned envelope (recordings + tags fetched from the
/// daemon) plus every `.wav` under the configured audio dir packed into
/// `audio/`. The GUI's plain JSON/CSV/TXT "Export All" carries no audio — this
/// is the one that round-trips with the CLI backup. Returns how many audio
/// files were packed so the caller can report it.
#[tauri::command]
pub async fn export_library_zip(bridge: Br<'_>, dest: String) -> Result<u64, CommandError> {
    reject_executable_dest(&dest)?;
    let recordings = forward(
        &bridge,
        Request::ListRecordings {
            filter: ListFilter::default(),
        },
    )
    .await?;
    // Tags are best-effort: a backup without the tag list is still useful.
    let tags = forward(&bridge, Request::ListTags)
        .await
        .unwrap_or_else(|_| serde_json::json!([]));

    let export_data = serde_json::json!({
        "version": 1,
        "recordings": recordings,
        "tags": tags,
    });
    let json_bytes = serde_json::to_vec_pretty(&export_data)
        .map_err(|e| CommandError::new("internal", format!("serializing catalog: {e}")))?;

    // The zip packing is synchronous file I/O (create + per-WAV read/deflate),
    // which on a large library would block an async worker thread — run it
    // off-runtime via spawn_blocking and await the result.
    tokio::task::spawn_blocking(move || -> Result<u64, CommandError> {
        use std::io::{Read, Write};
        use zip::write::SimpleFileOptions;

        let file = std::fs::File::create(&dest)
            .map_err(|e| CommandError::new("io", format!("creating {dest}: {e}")))?;
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("catalog.json", options)
            .map_err(|e| CommandError::new("io", format!("writing catalog.json: {e}")))?;
        zip.write_all(&json_bytes)
            .map_err(|e| CommandError::new("io", format!("writing catalog bytes: {e}")))?;

        // The audio dir is resolved tray-side (env-var/`~` expansion), the same
        // way the CLI does it — so the GUI backup packs the same files.
        let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
        let audio_dir_raw = cfg
            .expanded()
            .map(|c| c.recording.audio_dir)
            .unwrap_or_else(|_| cfg.recording.audio_dir.clone());
        let audio_dir = std::path::PathBuf::from(&audio_dir_raw);

        let mut packed: u64 = 0;
        if audio_dir.exists() {
            let mut stack = vec![audio_dir];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if !name.ends_with(".wav") {
                        continue;
                    }
                    if zip.start_file(format!("audio/{name}"), options).is_err() {
                        continue;
                    }
                    if let Ok(mut f) = std::fs::File::open(&path) {
                        let mut buf = Vec::new();
                        if f.read_to_end(&mut buf).is_ok() && zip.write_all(&buf).is_ok() {
                            packed += 1;
                        }
                    }
                }
            }
        }

        zip.finish()
            .map_err(|e| CommandError::new("io", format!("finalizing zip: {e}")))?;
        Ok(packed)
    })
    .await
    .map_err(|e| CommandError::new("internal", format!("spawn_blocking error: {e}")))?
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

/// Current capture status: `{ recording: bool, id: Option<String>, meeting: bool }`.
/// Lets the UI re-sync its record/meeting buttons after a reload, since the
/// daemon outlives the app window and a meeting may already be in progress.
#[tauri::command]
pub async fn record_status(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RecordStatus).await
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

/// Per-Library-kind recording counts (all / single / meeting / in-place /
/// favorite). Powers the sidebar's Library count badges.
#[tauri::command]
pub async fn kind_counts(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::KindCounts).await
}

/// Merge one tag into another: re-point all of `from_id`'s recordings onto
/// `into_id`, then delete `from_id`.
#[tauri::command]
pub async fn merge_tags(bridge: Br<'_>, from_id: i64, into_id: i64) -> Result<Value, CommandError> {
    forward(&bridge, Request::MergeTags { from_id, into_id }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── forward() with no bridge ───────────────────────────────────────────

    // ── render_captions (export_captions format→content mapping) ────────────

    fn cap_seg(start_ms: i64, end_ms: i64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start_ms,
            end_ms,
            text: text.to_string(),
            speaker: None,
        }
    }

    #[test]
    fn render_captions_srt_uses_comma_separator_and_cue_numbers() {
        let segs = [cap_seg(1000, 4500, "Hello world.")];
        let out = render_captions(&segs, CaptionFormat::Srt);
        // 1-based cue index, `HH:MM:SS,mmm` separator — the SRT shape.
        assert_eq!(out, "1\n00:00:01,000 --> 00:00:04,500\nHello world.\n");
    }

    #[test]
    fn render_captions_vtt_emits_header_and_dot_separator() {
        let segs = [cap_seg(1000, 4500, "Hello world.")];
        let out = render_captions(&segs, CaptionFormat::Vtt);
        // WEBVTT header + `HH:MM:SS.mmm` separator — distinct from SRT.
        assert_eq!(
            out,
            "WEBVTT\n\n00:00:01.000 --> 00:00:04.500\nHello world.\n\n"
        );
    }

    #[test]
    fn render_captions_formats_diverge_for_the_same_segments() {
        let segs = [cap_seg(0, 2000, "One."), cap_seg(2500, 5000, "Two.")];
        let srt = render_captions(&segs, CaptionFormat::Srt);
        let vtt = render_captions(&segs, CaptionFormat::Vtt);
        assert!(
            srt.starts_with('1'),
            "SRT starts with a cue number: {srt:?}"
        );
        assert!(
            vtt.starts_with("WEBVTT"),
            "VTT starts with its header: {vtt:?}"
        );
        assert_ne!(srt, vtt);
    }
}
