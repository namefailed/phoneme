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

/// Perform a semantic search across transcripts, optionally scoped (S3) by the
/// same Library filter as `list_recordings` (tag/status/date/kind/…). `filter`
/// omitted = unscoped (the prior behavior).
#[tauri::command]
pub async fn semantic_search(
    bridge: Br<'_>,
    query: String,
    limit: usize,
    filter: Option<ListFilter>,
) -> Result<Value, CommandError> {
    forward(
        &bridge,
        Request::SemanticSearch {
            query,
            limit,
            filter,
        },
    )
    .await
}

/// Clear all embeddings and re-embed the whole library with the current model
/// (run after changing the embedding model). Returns immediately; runs in the
/// background on the daemon.
#[tauri::command]
pub async fn reembed_all(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ReembedAll).await
}

/// Ask-my-archive (local RAG): answer a question from the user's own
/// transcripts, grounded with citations. The daemon ACKs immediately and
/// streams the answer over `DaemonEvent::AskActivity` (which rides the existing
/// `daemon-event` bridge to the webview), tagged with the frontend-minted
/// `request_id` so the chat panel can filter the shared event stream. `filter`
/// scopes the answer to a Library subset (same predicate semantics as
/// `semantic_search`); omit it for the whole library.
#[tauri::command]
pub async fn ask(
    bridge: Br<'_>,
    request_id: String,
    query: String,
    top_k: usize,
    filter: Option<ListFilter>,
) -> Result<Value, CommandError> {
    forward(
        &bridge,
        Request::Ask {
            request_id,
            query,
            top_k,
            filter,
        },
    )
    .await
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

/// Fetch a meeting's whole-meeting digest (the LLM synthesis across all tracks),
/// or `null` when none has been generated yet. The merged meeting view fetches it
/// alongside `list_meeting`.
#[tauri::command]
pub async fn get_meeting_digest(bridge: Br<'_>, meeting_id: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::GetMeetingDigest { meeting_id }).await
}

/// Fetch a stored period digest by its range `key` (the rollup across every
/// recording in a date window), or `null` when none has been generated for that
/// range. The digest panel fetches this by key.
#[tauri::command]
pub async fn get_period_digest(bridge: Br<'_>, key: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::GetPeriodDigest { key }).await
}

/// List every stored period digest, newest range first. Powers the digest
/// panel's history.
#[tauri::command]
pub async fn list_period_digests(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListPeriodDigests).await
}

/// Fetch one recording's machine transcript segments in timeline order
/// (start/end ms into the track's audio, text, optional speaker label). An
/// empty list is normal — older recordings predate segment capture and some
/// providers return no timing data. Powers the timeline views.
#[tauri::command]
pub async fn get_segments(
    bridge: Br<'_>,
    id: String,
    variant: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetSegments { id, variant }).await
}

/// Fetch one recording's auto-chapters in chronological order — a JSON array
/// (possibly empty) of `{ start_ms, end_ms, title, summary }`. An empty list is
/// normal (the recording has no timing to chapter, or the auto-chapter step never
/// ran). Powers the Chapters detail view.
#[tauri::command]
pub async fn get_chapters(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetChapters { id }).await
}

/// Fetch one recording's machine transcript words in timeline order — the
/// finer per-word layer beneath `get_segments`. Returns a JSON array (possibly
/// empty) of `{ idx, start_ms, end_ms, text, speaker, confidence }`, ordered by
/// `idx`; `confidence` is `null` when the provider gives none. An empty list is
/// normal (older recordings predate word capture, some providers emit no
/// per-word data). Fetched lazily by the word-level features (word seek,
/// confidence highlighting).
#[tauri::command]
pub async fn get_words(
    bridge: Br<'_>,
    id: String,
    variant: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetWords { id, variant }).await
}

/// List a recording's transcript versions — the compounding chain (PB-COMPOUND):
/// `idx` 0 = raw ASR, then each Transform step's output. Empty for a recording
/// that ran no Transform. Powers the Compare-versions step chain.
#[tauri::command]
pub async fn list_transcript_versions(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::ListTranscriptVersions { id }).await
}

/// Revert the live transcript to a recorded version (by step `idx`), through the
/// same path as a manual edit (re-flows the timing variants + re-embeds).
#[tauri::command]
pub async fn revert_to_version(
    bridge: Br<'_>,
    id: String,
    idx: i64,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::RevertToVersion { id, idx }).await
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
/// microphone and the system audio (WASAPI loopback) concurrently as two
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

/// Generate (or regenerate) a meeting's whole-meeting digest on demand — the
/// LLM synthesis across all of a meeting's tracks. `model` overrides the
/// configured summary model for this run only; `recipe_id` runs a specific meeting
/// template (a `scope = Meeting` recipe) for this run only. The digest arrives via
/// the `MeetingDigestUpdated` daemon event. Meeting-scope twin of `rerun_summary`.
#[tauri::command]
pub async fn rerun_meeting_digest(
    bridge: Br<'_>,
    meeting_id: String,
    model: Option<String>,
    recipe_id: Option<String>,
) -> Result<Value, CommandError> {
    forward(
        &bridge,
        Request::RerunMeetingDigest {
            meeting_id,
            model,
            recipe_id,
        },
    )
    .await
}

/// Generate (or regenerate) a period digest on demand — one LLM rollup across
/// every recording in a date window. `since`/`until` are RFC3339 timestamp
/// strings (the window bounds); `label` is the human period name; `model`
/// overrides the configured summary model for this run only. The digest arrives
/// via the `PeriodDigestUpdated` daemon event. Date-window twin of
/// `rerun_meeting_digest`.
///
/// The `since`/`until` strings are deserialized into the request's
/// `DateTime<Local>` fields through the wire schema (the same shape `ListFilter`
/// uses), so this command needs no direct chrono dependency. A malformed
/// timestamp yields a descriptive error rather than reaching the daemon.
#[tauri::command]
pub async fn rerun_period_digest(
    bridge: Br<'_>,
    since: String,
    until: String,
    label: String,
    model: Option<String>,
) -> Result<Value, CommandError> {
    let req: Request = serde_json::from_value(serde_json::json!({
        "type": "rerun_period_digest",
        "since": since,
        "until": until,
        "label": label,
        "model": model,
    }))
    .map_err(|e| CommandError::from(format!("invalid period digest range: {e}")))?;
    forward(&bridge, req).await
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

/// Dismiss a single item from the inbox `failed/` quarantine by id. Returns
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

/// Recent in-place dictations (the typed text) from the opt-in re-grab ring
/// buffer, newest first. Empty when `[in_place].keep_history` was never on.
#[tauri::command]
pub async fn list_dictation_history(bridge: Br<'_>, limit: u32) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListDictationHistory { limit }).await
}

/// Re-insert a past dictation's stored text at the current cursor (`mode` =
/// `"type"`/`"paste"`/omit for the configured `type_mode`). Errors `not_found`
/// for an unknown id.
#[tauri::command]
pub async fn regrab_dictation(
    bridge: Br<'_>,
    id: i64,
    mode: Option<String>,
) -> Result<Value, CommandError> {
    forward(&bridge, Request::RegrabDictation { id, mode }).await
}

/// Delete one dictation-history row by id (unknown ids are a no-op).
#[tauri::command]
pub async fn delete_dictation_history(bridge: Br<'_>, id: i64) -> Result<Value, CommandError> {
    forward(&bridge, Request::DeleteDictationHistory { id }).await
}

/// Empty the whole dictation-history ring buffer ("clear all").
#[tauri::command]
pub async fn clear_dictation_history(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ClearDictationHistory).await
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

/// Forget a named voice — reversibly (soft-delete; unlink its captures). Undo with
/// [`undo_forget_named_voice`].
#[tauri::command]
pub async fn forget_named_voice(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::ForgetNamedVoice { id }).await
}

/// Undo a [`forget_named_voice`] — restore the soft-deleted voice and re-link its
/// captures.
#[tauri::command]
pub async fn undo_forget_named_voice(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    forward(&bridge, Request::UndoForgetNamedVoice { id }).await
}

/// Remove every still-pending item from the queue ("clear queue").
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

/// Set or clear the "pinned" flag for a recording (Pinned view). Pinned
/// recordings sort to the top of the library, independent of the favorite flag.
#[tauri::command]
pub async fn set_pinned(bridge: Br<'_>, id: String, pinned: bool) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::SetPinned { id, pinned }).await
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

    let value = forward(&bridge, Request::GetSegments { id, variant: None }).await?;
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

/// Reject an export destination that would land inside a sensitive directory —
/// phoneme's own config dir or the Windows per-user auto-start (Startup) folder.
///
/// Defense-in-depth alongside [`reject_executable_dest`]: even a non-executable
/// file can be dangerous in the wrong place (a dropped `config.toml` next to the
/// daemon's, a `.url`/`.scr`-adjacent payload in Startup). The save dialog the
/// user drove is the real boundary, but a compromised WebView could try to push
/// a path here directly. We canonicalize the dest's parent (the dest itself
/// doesn't exist yet) and deny if it sits in a guarded root. Roots that can't be
/// resolved are simply skipped, so this only ever tightens — it never blocks a
/// write to a legitimate location.
fn reject_sensitive_dir_dest(dest: &str) -> Result<(), CommandError> {
    let parent = match std::path::Path::new(dest).parent() {
        // A bare filename (no parent) writes to the cwd — never a guarded root.
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => return Ok(()),
    };

    let mut guarded: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dirs) = directories::ProjectDirs::from("", "", "phoneme") {
        guarded.push(dirs.config_dir().to_path_buf());
    }
    // The Windows Startup folder: anything dropped here auto-runs at login.
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        guarded.push(
            std::path::Path::new(&appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup"),
        );
    }

    if guarded.iter().any(|root| super::path_within(&parent, root)) {
        return Err(CommandError::new(
            "invalid_config",
            "refusing to export into a protected directory (config / auto-start)",
        ));
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
/// The dest is screened by [`reject_executable_dest`] (no auto-run payload) and
/// [`reject_sensitive_dir_dest`] (no writing into the config / auto-start dirs).
#[tauri::command]
pub fn save_text_export(dest: String, contents: String) -> Result<(), CommandError> {
    reject_executable_dest(&dest)?;
    reject_sensitive_dir_dest(&dest)?;
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
    let segments = forward(
        &bridge,
        Request::GetSegments {
            id: rid,
            variant: None,
        },
    )
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

/// Export a time range of a recording's audio to a new WAV — the GUI entry
/// point for the same `ExportClip` request behind `phoneme clip`. `start_ms` /
/// `end_ms` are milliseconds from the recording's start; the daemon slices
/// `[start, end)` on sample-frame boundaries and clamps `end` to the recording's
/// duration, so an `end` past the audio is fine (the CLI relies on the same
/// clamp). `out_path` is `None`/empty to let the daemon pick the sibling
/// `_clip_<start>-<end>.wav` path next to the source. Returns the daemon's
/// `{ "path": "…" }` for the WebView to toast. A thin forwarder — every check
/// (id validity, non-empty range, dest ≠ source) lives in the daemon handler.
#[tauri::command]
pub async fn export_clip(
    bridge: Br<'_>,
    id: String,
    start_ms: i64,
    end_ms: i64,
    out_path: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(
        &bridge,
        Request::ExportClip {
            id,
            start_ms,
            end_ms,
            out_path,
        },
    )
    .await
}

/// Zip-entry name for one audio file under `audio_dir`, preserving its day
/// folder. WAVs live at `<audio_dir>/<YYYY-MM-DD>/<HHmmssMMM>.wav` and the stem
/// is time-of-day only, so two recordings at the same ms-of-day on different
/// days share a stem. Naming the entry from the path relative to `audio_dir`
/// (backslashes normalized to `/` for a portable archive) keeps the day folder,
/// so the two never collapse to one entry and clobber each other on restore.
/// Falls back to the bare filename if the path isn't under `audio_dir`.
fn audio_zip_entry_name(audio_dir: &std::path::Path, path: &std::path::Path) -> String {
    match path.strip_prefix(audio_dir) {
        Ok(rel) => format!("audio/{}", rel.to_string_lossy().replace('\\', "/")),
        Err(_) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("audio/{name}")
        }
    }
}

/// Write a portable backup of the whole library to `dest` (a `.zip` path the
/// WebView picked via the save dialog). Mirrors `phoneme export <FILE>`: a
/// `catalog.json` versioned envelope (recordings + tags + whole-meeting
/// digests fetched from the daemon) plus every `.wav` under the configured
/// audio dir packed into `audio/`. The GUI's plain JSON/CSV/TXT "Export All"
/// carries no audio — this is the one that round-trips with the CLI backup.
/// Returns how many audio files were packed so the caller can report it.
#[tauri::command]
pub async fn export_library_zip(bridge: Br<'_>, dest: String) -> Result<u64, CommandError> {
    reject_executable_dest(&dest)?;
    reject_sensitive_dir_dest(&dest)?;
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
    // Whole-meeting digests live in their own side table (keyed by meeting_id),
    // so the per-recording list doesn't carry them — fetch them separately so
    // they round-trip. Best-effort like the tags.
    let meeting_digests = forward(&bridge, Request::ListMeetingDigests)
        .await
        .unwrap_or_else(|_| serde_json::json!([]));

    let export_data = serde_json::json!({
        "version": 1,
        "recordings": recordings,
        "tags": tags,
        "meeting_digests": meeting_digests,
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
            let mut stack = vec![audio_dir.clone()];
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
                    // Entry name preserves the day folder so two same-ms-
                    // different-day recordings don't collide — see
                    // `audio_zip_entry_name`.
                    let entry_name = audio_zip_entry_name(&audio_dir, &path);
                    if zip.start_file(entry_name, options).is_err() {
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

/// Run the LLM entity-extraction step for one recording on demand. The
/// structured entities land on the recording and arrive via the
/// `EntitiesUpdated` daemon event. Entity counterpart of `suggest_tags`.
#[tauri::command]
pub async fn suggest_entities(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::SuggestEntities { id }).await
}

/// Run the LLM auto-chapter step for one recording on demand. The time-ranged
/// chapters land on the recording and arrive via the `ChaptersUpdated` daemon
/// event. Chapter counterpart of `suggest_entities`.
#[tauri::command]
pub async fn suggest_chapters(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::SuggestChapters { id }).await
}

/// Run the LLM task-extraction step for one recording on demand. The structured
/// tasks land on the recording (preserving any `done` flag on a surviving task)
/// and arrive via the `TasksUpdated` daemon event. Task counterpart of
/// `suggest_entities`.
#[tauri::command]
pub async fn suggest_tasks(bridge: Br<'_>, id: String) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::SuggestTasks { id }).await
}

/// Toggle (or set) one task's done flag. Emits `TasksUpdated` for the recording
/// so open views refresh. `not_found` when `task_id` is unknown.
#[tauri::command]
pub async fn set_task_done(
    bridge: Br<'_>,
    id: String,
    task_id: i64,
    done: bool,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::SetTaskDone { id, task_id, done }).await
}

/// Add a user-created task to a recording. Emits `TasksUpdated`.
#[tauri::command]
pub async fn add_task(
    bridge: Br<'_>,
    id: String,
    text: String,
    due_hint: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::AddTask { id, text, due_hint }).await
}

/// Edit one task's text (and optional due hint). `not_found` when unknown.
#[tauri::command]
pub async fn update_task(
    bridge: Br<'_>,
    id: String,
    task_id: i64,
    text: String,
    due_hint: Option<String>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(
        &bridge,
        Request::UpdateTask {
            id,
            task_id,
            text,
            due_hint,
        },
    )
    .await
}

/// Delete one task. `not_found` when unknown.
#[tauri::command]
pub async fn delete_task(bridge: Br<'_>, id: String, task_id: i64) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::DeleteTask { id, task_id }).await
}

/// Set the user's task order for a recording (drag-reorder). Emits `TasksUpdated`.
#[tauri::command]
pub async fn reorder_tasks(
    bridge: Br<'_>,
    id: String,
    task_ids: Vec<i64>,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::ReorderTasks { id, task_ids }).await
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

/// Reassign one transcript segment to a different speaker label (U1). `idx` is
/// the 0-based segment index from `get_segments`; `new_label` is the 1-based
/// `[Speaker N]` index (a brand-new label simply starts existing). Segments stay
/// authoritative and the prose markers are rebuilt to match; a `SpeakerNameUpdated`
/// event fires so the detail view refreshes.
#[tauri::command]
pub async fn reassign_segment_speaker(
    bridge: Br<'_>,
    id: String,
    idx: i64,
    new_label: i64,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(
        &bridge,
        Request::ReassignSegmentSpeaker { id, idx, new_label },
    )
    .await
}

/// Merge two speakers in a recording (U1): every `from_label` segment becomes
/// `into_label`, then `from_label` ceases to exist. `into` keeps its name (adopts
/// `from`'s only when unnamed); `from`'s voiceprint is dropped and any affected
/// named voice recomputed. Fires `SpeakerNameUpdated`.
#[tauri::command]
pub async fn merge_speakers(
    bridge: Br<'_>,
    id: String,
    from_label: i64,
    into_label: i64,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(
        &bridge,
        Request::MergeSpeakers {
            id,
            from_label,
            into_label,
        },
    )
    .await
}

/// Split some of a speaker's segments off onto a fresh label (U1). The listed
/// `segment_idxs` move from `label` to `new_label` (which starts with no
/// name/voiceprint); every other segment of `label` stays. Fires
/// `SpeakerNameUpdated`.
#[tauri::command]
pub async fn split_speaker(
    bridge: Br<'_>,
    id: String,
    label: i64,
    segment_idxs: Vec<i64>,
    new_label: i64,
) -> Result<Value, CommandError> {
    let id = parse_id(&id)?;
    forward(
        &bridge,
        Request::SplitSpeaker {
            id,
            label,
            segment_idxs,
            new_label,
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

/// Return every tag, including orphaned ones with no recordings attached.
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

/// The cross-recording entity facet: every distinct extracted entity across the
/// library with its recording count. Powers the sidebar's browse-by-entity
/// surface (the entity counterpart of `list_all_tags` + `tag_usage_counts`).
#[tauri::command]
pub async fn list_all_entities(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListAllEntities).await
}

/// The cross-recording task list: every extracted task across the library, open
/// first. Powers the sidebar's Tasks section. When `only_open` is set, done tasks
/// are dropped. The task counterpart of `list_all_entities`.
#[tauri::command]
pub async fn list_all_tasks(bridge: Br<'_>, only_open: bool) -> Result<Value, CommandError> {
    forward(&bridge, Request::ListAllTasks { only_open }).await
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

    // ── audio_zip_entry_name (H1 backup collision) ──────────────────────────

    #[test]
    fn zip_entry_keeps_the_day_folder() {
        let dir = std::path::Path::new("/data/audio");
        let path = std::path::Path::new("/data/audio/2026-05-19/143500042.wav");
        assert_eq!(
            audio_zip_entry_name(dir, path),
            "audio/2026-05-19/143500042.wav"
        );
    }

    #[test]
    fn zip_entry_distinguishes_same_ms_different_day() {
        // H1: two recordings at the same ms-of-day on different days share a
        // `143500042.wav` stem. A flat entry name collapses them into one and
        // loses a recording on restore; preserving the day folder keeps both.
        let dir = std::path::Path::new("/data/audio");
        let a = audio_zip_entry_name(
            dir,
            std::path::Path::new("/data/audio/2026-05-19/143500042.wav"),
        );
        let b = audio_zip_entry_name(
            dir,
            std::path::Path::new("/data/audio/2026-05-20/143500042.wav"),
        );
        assert_ne!(a, b, "same-ms-different-day files must not collide");
    }

    #[test]
    fn zip_entry_uses_forward_slashes() {
        // Portable archives use `/` even when strip_prefix yields a Windows
        // `2026-05-19\143500042.wav` relative path.
        let dir = std::path::Path::new(r"C:\data\audio");
        let path = std::path::Path::new(r"C:\data\audio\2026-05-19\143500042.wav");
        let name = audio_zip_entry_name(dir, path);
        assert!(!name.contains('\\'), "no backslashes: {name}");
        assert_eq!(name, "audio/2026-05-19/143500042.wav");
    }

    // ── reject_sensitive_dir_dest (save_text_export defense-in-depth) ───────

    #[test]
    fn sensitive_dir_check_allows_an_ordinary_destination() {
        // A normal save target (an existing temp dir) is fine — the guard only
        // tightens, it must never block a legitimate export.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("transcript.txt");
        assert!(reject_sensitive_dir_dest(&dest.to_string_lossy()).is_ok());
    }

    #[test]
    fn sensitive_dir_check_allows_a_bare_filename() {
        // No parent component → writes to the cwd, which is never a guarded root.
        assert!(reject_sensitive_dir_dest("transcript.txt").is_ok());
    }

    #[test]
    fn sensitive_dir_check_rejects_the_config_dir() {
        // A write whose parent canonicalizes inside phoneme's config dir is
        // denied. Skip if the config dir doesn't exist on this box (canonicalize
        // would fail-closed and the guard couldn't fire — nothing to assert).
        let Some(dirs) = directories::ProjectDirs::from("", "", "phoneme") else {
            return;
        };
        let cfg_dir = dirs.config_dir().to_path_buf();
        if std::fs::create_dir_all(&cfg_dir).is_err() {
            return;
        }
        let dest = cfg_dir.join("config.toml");
        assert!(
            reject_sensitive_dir_dest(&dest.to_string_lossy()).is_err(),
            "a write into the config dir must be refused"
        );
    }
}
