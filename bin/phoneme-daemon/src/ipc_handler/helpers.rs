//! Small response/error helpers split out of the dispatch in `super`.
use super::*;

pub(super) fn error_to_kind(e: &phoneme_core::Error) -> IpcErrorKind {
    use phoneme_core::Error::*;
    match e {
        AlreadyRecording { .. } => IpcErrorKind::AlreadyRecording,
        NotRecording => IpcErrorKind::NotRecording,
        NotFound { .. } => IpcErrorKind::NotFound,
        InvalidConfig(_) => IpcErrorKind::InvalidConfig,
        WhisperUnreachable { .. } => IpcErrorKind::WhisperUnreachable,
        WhisperTimeout { .. } => IpcErrorKind::WhisperTimeout,
        HookFailed { .. } | HookTimeout { .. } => IpcErrorKind::HookFailed,
        DaemonNotRunning => IpcErrorKind::DaemonNotRunning,
        PipeInUse { .. } => IpcErrorKind::PipeInUse,
        ShuttingDown => IpcErrorKind::ShuttingDown,
        Io(_) => IpcErrorKind::Io,
        _ => IpcErrorKind::Internal,
    }
}

/// The error arm shared by nearly every handler: a core error answered as
/// `Response::Err` with the standard kind mapping and the error's own text.
/// Wire-identical to spelling the `IpcError` out at the call site.
/// The (mode, threshold) pair the voiceprint scorer should use for a diarization
/// config — V2 score-norm aware. With norm `off` (default) it's the raw cosine
/// bar; with `s_norm`/`as_norm` it's the z-score bar. Shared by recognition and V5
/// propagation so both judge "is this the same voice" the same way.
pub(super) fn voiceprint_scorer(
    diar: &phoneme_core::config::DiarizationConfig,
) -> (phoneme_core::voiceprint::ScoreNorm, f32) {
    let mode = phoneme_core::voiceprint::ScoreNorm::from(diar.voiceprint_score_norm);
    let threshold = if mode == phoneme_core::voiceprint::ScoreNorm::Off {
        diar.voiceprint_match_threshold as f32
    } else {
        diar.voiceprint_score_norm_threshold as f32
    };
    (mode, threshold)
}

/// Run V5 name propagation for a just-enrolled named voice, returning the JSON the
/// `SetSpeakerName` response carries. Routes on `diar.name_propagation`: `off` is a
/// no-op; `auto` back-fills every candidate and reports the count; `ask` returns
/// the candidate list for the UI to confirm (applying nothing). Best-effort — any
/// catalog error is logged and reported as an empty result, never failing the
/// rename.
pub(super) async fn speaker_name_propagation(
    state: &AppState,
    named_voice_id: &str,
    diar: &phoneme_core::config::DiarizationConfig,
) -> serde_json::Value {
    use phoneme_core::config::NamePropagation;
    if diar.name_propagation == NamePropagation::Off {
        return serde_json::json!({ "policy": "off", "applied": 0, "candidates": [] });
    }
    let (mode, threshold) = voiceprint_scorer(diar);
    let candidates = match state
        .catalog
        .propagation_candidates(named_voice_id, threshold, mode)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(voice = %named_voice_id, "propagation candidate scan failed: {e}");
            // Report the policy that was actually configured (not a hardcoded
            // tag) and flag the failure so callers can tell an empty result
            // apart from a clean "no candidates" one.
            return serde_json::json!({
                "policy": format!("{:?}", diar.name_propagation).to_lowercase(),
                "applied": 0,
                "candidates": [],
                "error": true,
            });
        }
    };
    match diar.name_propagation {
        NamePropagation::Off => unreachable!("handled above"),
        NamePropagation::Auto => {
            let targets: Vec<(phoneme_core::id::RecordingId, i64)> = candidates
                .iter()
                .map(|c| (c.recording_id.clone(), c.speaker_label))
                .collect();
            let (applied, apply_err) = match state
                .catalog
                .apply_propagation(named_voice_id, &targets)
                .await
            {
                Ok(applied) => (applied, false),
                Err(e) => {
                    tracing::warn!(voice = %named_voice_id, "propagation apply failed: {e}");
                    (Vec::new(), true)
                }
            };
            // Nudge clients to refresh only the recordings actually back-filled,
            // not every candidate (many are skipped: already named, no voiceprint).
            let mut refreshed: std::collections::HashSet<phoneme_core::id::RecordingId> =
                std::collections::HashSet::new();
            for (rid, _label) in &applied {
                if refreshed.insert(rid.clone()) {
                    state
                        .events
                        .emit(DaemonEvent::SpeakerNameUpdated { id: rid.clone() });
                }
            }
            // Flag the failure so an aborted back-fill isn't read as a clean
            // "applied 0" — the policy tag itself stays the real one.
            serde_json::json!({
                "policy": format!("{:?}", diar.name_propagation).to_lowercase(),
                "applied": applied.len(),
                "candidates": [],
                "error": apply_err,
            })
        }
        NamePropagation::Ask => {
            // Surface the candidates for the UI to confirm; change nothing now.
            serde_json::json!({
                "policy": "ask",
                "applied": 0,
                "candidates": serde_json::to_value(&candidates).unwrap_or(serde_json::Value::Array(vec![])),
            })
        }
    }
}

/// Drop any "cleaned" timing layer (TL-CONSISTENCY) for a recording. A transcript
/// edit or a speaker correction invalidates it — the cleaned layer was aligned to
/// the pre-change text/labels — so clearing it makes the Synced/Timeline views fall
/// back to the (now-correct) raw layer until a retranscribe rebuilds the cleaned one.
/// Best-effort: a failure just leaves a stale cleaned layer.
pub(super) async fn clear_cleaned_timing(state: &AppState, id: &phoneme_core::RecordingId) {
    if let Err(e) = state
        .catalog
        .replace_words_variant(id, "cleaned", &[])
        .await
    {
        tracing::warn!(id = %id.as_str(), error = %e, "failed to clear cleaned words");
    }
    if let Err(e) = state
        .catalog
        .replace_segments_variant(id, "cleaned", &[])
        .await
    {
        tracing::warn!(id = %id.as_str(), error = %e, "failed to clear cleaned segments");
    }
}

/// Post-edit upkeep shared by `UpdateTranscript` and `FindReplace`: re-flow the
/// per-word and per-segment timing layers onto `new_text` (so the Synced/Timeline
/// views follow the edit), re-embed the new text for semantic search, then emit
/// `TranscriptUpdated`. Best-effort throughout: the prose is already persisted by
/// the caller, so a re-align or re-embed failure is logged, not surfaced. The
/// re-flow is gated by `editor.resync_views_on_edit`, an opt-out for users who
/// prefer the original machine timings.
pub(super) async fn reflow_and_reembed_after_edit(
    state: &AppState,
    id: &phoneme_core::RecordingId,
    new_text: &str,
) {
    let cfg = state.config.load();
    if cfg.editor.resync_views_on_edit {
        match state.catalog.words_for(id).await {
            Ok(old_words) => {
                if let Some(r) = phoneme_core::realign::realign_transcript(new_text, &old_words) {
                    let words_stored = match state.catalog.replace_words(id, &r.words).await {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::warn!(id = %id, error = %e, "re-align: failed to store re-flowed words");
                            false
                        }
                    };
                    if let Err(e) = state.catalog.replace_segments(id, &r.segments).await {
                        tracing::warn!(id = %id, error = %e, "re-align: failed to store re-flowed segments");
                    }
                    // The re-flow changed the per-word layer — hand-edited words
                    // typically drop to NULL confidence — so the row-level
                    // `mean_confidence` aggregate is now stale. Recompute it from
                    // the re-flowed words against the live threshold so the badge
                    // and the low-confidence filter stay correct (the migration
                    // invariant: `mean_confidence` mirrors the stored words). Only
                    // when those words actually persisted, so the aggregate can
                    // never reflect a word layer that failed to write. Best-effort.
                    if words_stored {
                        let mean = phoneme_core::ConfidenceAggregate::compute(
                            &r.words,
                            cfg.whisper.low_confidence_threshold,
                        )
                        .map(|a| a.mean);
                        if let Err(e) = state.catalog.update_confidence(id, mean).await {
                            tracing::warn!(id = %id, error = %e, "re-align: failed to refresh mean_confidence");
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(id = %id, error = %e, "re-align: could not load words; leaving timing layers untouched");
            }
        }
        // The raw layer above is now re-flowed onto `new_text`, so the cleaned
        // layer (aligned to the pre-edit text) is stale — drop it. Manual edit,
        // find-replace, and revert-to-version all land here.
        clear_cleaned_timing(state, id).await;
    }

    let embedder = state.embedder.read().await.as_ref().cloned();
    if let Some(embedder) = embedder {
        crate::pipeline::embed_and_store(embedder, &state.catalog, id, new_text).await;
    }

    state
        .events
        .emit(DaemonEvent::TranscriptUpdated { id: id.clone() });
}

pub(super) fn err_response(e: &phoneme_core::Error) -> Response {
    Response::Err(IpcError {
        kind: error_to_kind(e),
        message: e.to_string(),
    })
}

/// A `NotFound` error response. Callers format the message — the wording varies
/// per request and is part of the wire contract — and this pins the kind.
pub(super) fn not_found(message: String) -> Response {
    Response::Err(IpcError {
        kind: IpcErrorKind::NotFound,
        message,
    })
}

/// The bare `Ok(null)` acknowledgement most mutating requests answer with.
pub(super) fn ok_null() -> Response {
    Response::Ok(serde_json::Value::Null)
}

/// On-demand enrichment (Extract entities/tasks/chapters) hit with no usable LLM
/// provider. Unlike the auto pipeline — which silently skips so a missing model
/// never fails a recording — an explicit Extract click must tell the user why
/// nothing happened, so the frontend can toast this instead of looking dead.
pub(super) fn no_provider_response(provider: &str) -> Response {
    Response::Err(IpcError {
        kind: IpcErrorKind::InvalidConfig,
        message: format!(
            "No usable AI provider configured (provider \"{provider}\"). Set one under \
             Settings → Post-Processing (or this step's Playbook entry) to use Extract."
        ),
    })
}

pub(super) fn serialize_response<T: serde::Serialize>(val: T) -> Response {
    match serde_json::to_value(val) {
        Ok(v) => Response::Ok(v),
        Err(e) => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: format!("serialization failed: {e}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_recipe_validation() {
        // The seeded config has `default` (recording) + `meeting_digest`/`standup`/
        // `interview` (meeting) recipes.
        let cfg = phoneme_core::Config::default();
        // None / empty / whitespace → no override (the global default).
        assert_eq!(validate_import_recipe(&cfg, None).unwrap(), None);
        assert_eq!(
            validate_import_recipe(&cfg, Some("   ".into())).unwrap(),
            None
        );
        // A recording-scope recipe passes through (trimmed).
        assert_eq!(
            validate_import_recipe(&cfg, Some(" default ".into())).unwrap(),
            Some("default".into())
        );
        // An unknown id is lenient (pipeline falls back to default), not an error.
        assert_eq!(
            validate_import_recipe(&cfg, Some("nope".into())).unwrap(),
            Some("nope".into())
        );
        // A meeting template is rejected — a single import is not a meeting.
        let err = validate_import_recipe(&cfg, Some("meeting_digest".into())).unwrap_err();
        assert!(err.contains("meeting_digest"), "names the recipe: {err}");
        assert!(err.contains("scope = Meeting"), "explains why: {err}");
    }

    #[test]
    fn reimport_id_from_path_parts_round_trips() {
        // The real audio_dir layout: day folder + 9-char time stem -> 18-char id.
        let id = id_from_path_parts("2026-06-15", "014341016").unwrap();
        assert_eq!(id.as_str(), "20260615T014341016");
        assert_eq!(id.day_folder(), "2026-06-15");
        assert_eq!(id.file_stem(), "014341016");
        // A user-dropped file with a non-id name is skipped, not mis-relinked.
        assert!(id_from_path_parts("2026-06-15", "my-notes").is_none());
        assert!(id_from_path_parts("not-a-day", "014341016").is_none());
    }

    #[test]
    fn reimport_started_at_decodes_the_id_timestamp() {
        use chrono::{Datelike, Timelike};
        let id = phoneme_core::RecordingId::parse("20260615T014341016").unwrap();
        let dt = started_at_from_id(&id);
        assert_eq!((dt.year(), dt.month(), dt.day()), (2026, 6, 15));
        assert_eq!((dt.hour(), dt.minute(), dt.second()), (1, 43, 41));
    }

    #[test]
    fn import_size_cap_rejects_oversized_files() {
        // At or below the cap is accepted; one byte over is rejected.
        assert!(!exceeds_import_size_cap(0));
        assert!(!exceeds_import_size_cap(MAX_IMPORT_BYTES));
        assert!(exceeds_import_size_cap(MAX_IMPORT_BYTES + 1));
        // A clearly-oversized file (3 GiB) is rejected.
        assert!(exceeds_import_size_cap(3 * 1024 * 1024 * 1024));
    }

    #[test]
    fn hook_allowlist_accepts_only_configured_commands() {
        let configured = vec![
            "powershell -File C:\\hooks\\save.ps1".to_string(),
            "  notify-send {transcript}  ".to_string(), // padded in config
        ];
        // Exact configured command is allowed.
        assert!(hook_command_allowed(
            "powershell -File C:\\hooks\\save.ps1",
            &configured
        ));
        // Whitespace differences around the command don't matter (trimmed both sides).
        assert!(hook_command_allowed(
            "notify-send {transcript}",
            &configured
        ));
        // A command not in the list is rejected — this is the exec channel we close.
        assert!(!hook_command_allowed("calc.exe", &configured));
        assert!(!hook_command_allowed(
            "powershell -Command Remove-Item C:\\ -Recurse",
            &configured
        ));
        // Empty / whitespace-only requests are never allowed.
        assert!(!hook_command_allowed("", &configured));
        assert!(!hook_command_allowed("   ", &configured));
        // With no configured hooks, nothing is allowed.
        assert!(!hook_command_allowed("anything", &[]));
    }

    #[test]
    fn audio_path_guard_only_accepts_paths_under_audio_dir() {
        let dir = std::path::Path::new("/data/phoneme/audio");
        // A normal recording path under the audio dir is accepted.
        assert!(audio_path_is_ours(
            "/data/phoneme/audio/2026-06-08/rec.wav",
            dir
        ));
        // The audio dir itself is trivially "under" itself.
        assert!(audio_path_is_ours("/data/phoneme/audio", dir));
        // Paths outside the audio dir are rejected.
        assert!(!audio_path_is_ours("/etc/passwd", dir));
        // A sibling that merely shares a name prefix is rejected (component-wise
        // starts_with, not a string prefix).
        assert!(!audio_path_is_ours("/data/phoneme/audio-evil/x.wav", dir));
        // `..` traversal that would climb out is rejected even if it textually
        // begins under the audio dir.
        assert!(!audio_path_is_ours(
            "/data/phoneme/audio/../../etc/passwd",
            dir
        ));
    }

    // ── RetranscribeRecording model override (#49 regression) ──────────────

    use crate::app_state::AppState;
    use phoneme_core::config::{Config, TranscriptionBackend, WhisperMode};
    use phoneme_core::types::{Recording, RecordingStatus};
    use phoneme_core::RecordingId;

    async fn override_test_state(tmp: &std::path::Path, cfg: Config) -> AppState {
        // Explicit data-local (no global `set_var`) so parallel tests don't race —
        // see `AppState::new_in`.
        AppState::new_in(cfg, Some(tmp.join("data")))
            .await
            .expect("build test AppState")
    }

    /// `daemon_status` surfaces the bundled-server ports: the configured
    /// (preferred) one and the one the supervisor actually bound, so clients
    /// probing the local server dial it even after a port fallback.
    #[tokio::test]
    async fn daemon_status_reports_preferred_and_effective_ports() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.bundled_server_port = 5809;
        // A dedicated preview server is configured on its own port, so the
        // preview-port fields aren't null-by-default — they carry the configured
        // (preferred) value and, once published, the bound (effective) one.
        let mut preview = cfg.whisper.clone();
        preview.bundled_server_port = 5810;
        cfg.preview_whisper = Some(preview);
        let state = override_test_state(tmp.path(), cfg).await;

        // Servers not (yet) running: each preferred mirrors config, each effective null.
        let Response::Ok(v) = handle_request(Request::DaemonStatus, &state).await else {
            panic!("daemon_status should answer ok");
        };
        assert_eq!(v["whisper_preferred_port"], 5809);
        assert!(v["whisper_effective_port"].is_null());
        assert_eq!(v["preview_whisper_preferred_port"], 5810);
        assert!(v["preview_whisper_effective_port"].is_null());

        // The supervisor published a fallback port for each server: effective
        // reports it while preferred keeps naming the configured value. The main
        // and preview ports must not cross-wire — each effective field carries its
        // own server's bound port.
        state.whisper_ports.set_main(Some(51234));
        state.whisper_ports.set_preview(Some(52345));
        let Response::Ok(v) = handle_request(Request::DaemonStatus, &state).await else {
            panic!("daemon_status should answer ok");
        };
        assert_eq!(v["whisper_preferred_port"], 5809);
        assert_eq!(v["whisper_effective_port"], 51234);
        assert_eq!(v["preview_whisper_preferred_port"], 5810);
        assert_eq!(v["preview_whisper_effective_port"], 52345);
    }

    /// Insert a minimal Done recording row so a retranscribe has something to act
    /// on, and return its id.
    async fn insert_done_recording(state: &AppState) -> RecordingId {
        let id = RecordingId::new();
        let row = Recording {
            id: id.clone(),
            started_at: chrono::Local::now(),
            duration_ms: 1000,
            audio_path: "C:/phoneme/audio/x.wav".into(),
            transcript: Some("hello".into()),
            model: Some("ggml-base.en".into()),
            status: RecordingStatus::Done,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            pinned: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            entities_model: None,
            chapters_model: None,
            tasks_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            mean_confidence: None,
            detected_language: None,
            ext_ref: None,
            tags: vec![],
            entities: vec![],
            tasks: vec![],
            speaker_names: vec![],
        };
        state.catalog.insert(&row).await.unwrap();
        id
    }

    /// Guards #49: a model-override re-transcription for a Local (bundled)
    /// recording must not mutate the process-global whisper config. Writing the
    /// override model into the shared config makes the whisper supervisor (which
    /// polls it) restart, and the queue worker's post-run reload reverts it and
    /// restarts again. That double restart races every other queued/preview
    /// transcription (which reads the same global config) and mass-fails them. The
    /// override must instead be recorded for just this one job in
    /// `pending_overrides`.
    #[tokio::test]
    async fn model_override_retranscribe_does_not_mutate_global_config() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = "C:/models/ggml-base.en.bin".into();
        cfg.whisper.bundled_server_port = 5809;
        let state = override_test_state(tmp.path(), cfg).await;

        let id = insert_done_recording(&state).await;
        // Snapshot the configured model before the request.
        let model_path_before = state.config.load().whisper.model_path.clone();
        let port_before = state.config.load().whisper.bundled_server_port;

        let resp = handle_request(
            Request::RetranscribeRecording {
                id: id.clone(),
                model: Some("C:/models/ggml-large-v3.bin".into()),
                run_hooks: None,
                post_process: None,
                all_overrides: None,
                recipe_id: None,
            },
            &state,
        )
        .await;
        assert!(
            matches!(resp, Response::Ok(_)),
            "retranscribe should be accepted"
        );

        // The global config is untouched — the crux of the fix. The supervisor
        // never sees a model change here, so it never thrashes.
        let after = state.config.load();
        assert_eq!(
            after.whisper.model_path, model_path_before,
            "global whisper.model_path must NOT change on a model-override retranscribe"
        );
        assert_eq!(
            after.whisper.bundled_server_port, port_before,
            "global whisper port must be unchanged"
        );

        // The override is instead recorded against just this recording id, to be
        // applied by the pipeline when this single job runs. (Scoped so the std
        // MutexGuard drops before the await below — clippy::await_holding_lock.)
        {
            let pending = state.pending_overrides.lock().unwrap();
            assert_eq!(
                pending.get(&id).map(String::as_str),
                Some("C:/models/ggml-large-v3.bin"),
                "the per-job override should be queued for this recording only"
            );
        }

        // And the recording was put back into the queue (Queued; the worker
        // flips it to Transcribing when it claims the item) + enqueued.
        let rec = state.catalog.get(&id).await.unwrap().unwrap();
        assert_eq!(rec.status, RecordingStatus::Queued);
    }

    /// The Shutdown handler must reply before the daemon exits: the Ok is produced
    /// immediately while the coordinator trigger lags by the grace delay, so the
    /// caller (`phoneme daemon stop`, the tray's Quit) always reads its
    /// acknowledgement off the pipe before teardown begins.
    #[tokio::test]
    async fn shutdown_replies_before_triggering_the_coordinator() {
        let tmp = tempfile::tempdir().unwrap();
        let state = override_test_state(tmp.path(), Config::default()).await;

        let resp = handle_request(Request::Shutdown, &state).await;
        assert!(matches!(resp, Response::Ok(_)), "shutdown must ACK");
        assert!(
            !state.shutdown.signal.is_shutting_down(),
            "the trigger must lag the reply (grace delay), not race it"
        );

        // ...and the trigger must actually arrive shortly after the grace.
        let mut signal = state.shutdown.signal.clone();
        tokio::time::timeout(std::time::Duration::from_secs(5), signal.wait())
            .await
            .expect("shutdown must trigger after the grace delay");
    }

    /// A retranscribe WITHOUT a model override must not create a phantom override
    /// entry (so a plain re-run always uses the configured model).
    #[tokio::test]
    async fn retranscribe_without_model_records_no_override() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = "C:/models/ggml-base.en.bin".into();
        let state = override_test_state(tmp.path(), cfg).await;

        let id = insert_done_recording(&state).await;
        let resp = handle_request(
            Request::RetranscribeRecording {
                id: id.clone(),
                model: None,
                run_hooks: Some(false),
                post_process: Some(false),
                all_overrides: None,
                recipe_id: None,
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok(_)));
        assert!(
            state.pending_overrides.lock().unwrap().get(&id).is_none(),
            "no model override should be recorded when none was requested"
        );
    }

    /// Cancelling a queued item must mark the recording `Cancelled` — NOT
    /// `TranscribeFailed`. A user removing their own item from the queue is not
    /// a failure: the old status lit the failed badge and listed the recording
    /// in the failure panel for something the user did on purpose.
    #[tokio::test]
    async fn cancel_queued_marks_recording_cancelled_not_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = override_test_state(tmp.path(), Config::default()).await;

        // A recording waiting in the queue: catalog row at Queued plus a
        // pending inbox payload (what RecordStop / import leave behind).
        let id = insert_done_recording(&state).await;
        state
            .catalog
            .update_status(&id, RecordingStatus::Queued)
            .await
            .unwrap();
        let payload = phoneme_core::HookPayload {
            id: id.clone(),
            timestamp: chrono::Local::now(),
            transcript: String::new(),
            audio_path: "C:/phoneme/audio/x.wav".into(),
            duration_ms: 1000,
            model: String::new(),
            metadata: HookMetadata::current(),
        };
        state.inbox.enqueue(&payload).await.unwrap();

        let resp = handle_request(Request::CancelQueued { id: id.clone() }, &state).await;
        assert!(matches!(resp, Response::Ok(_)), "cancel should succeed");

        let rec = state.catalog.get(&id).await.unwrap().unwrap();
        assert_eq!(
            rec.status,
            RecordingStatus::Cancelled,
            "a user cancel is Cancelled, never a failed status"
        );
    }

    /// CancelAllQueued ("clear queue") marks every removed item `Cancelled`,
    /// mirroring the single-item path.
    #[tokio::test]
    async fn cancel_all_queued_marks_recordings_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        let state = override_test_state(tmp.path(), Config::default()).await;

        let mut ids = Vec::new();
        for _ in 0..2 {
            let id = insert_done_recording(&state).await;
            state
                .catalog
                .update_status(&id, RecordingStatus::Queued)
                .await
                .unwrap();
            let payload = phoneme_core::HookPayload {
                id: id.clone(),
                timestamp: chrono::Local::now(),
                transcript: String::new(),
                audio_path: "C:/phoneme/audio/x.wav".into(),
                duration_ms: 1000,
                model: String::new(),
                metadata: HookMetadata::current(),
            };
            state.inbox.enqueue(&payload).await.unwrap();
            ids.push(id);
        }

        let resp = handle_request(Request::CancelAllQueued, &state).await;
        let Response::Ok(v) = resp else {
            panic!("cancel-all should succeed");
        };
        assert_eq!(v["removed"], 2);
        for id in &ids {
            let rec = state.catalog.get(id).await.unwrap().unwrap();
            assert_eq!(rec.status, RecordingStatus::Cancelled);
        }
    }

    /// HookTest output crosses the pipe to the tray/CLI, and the test command is
    /// caller-supplied — a script that dumps its environment must not hand
    /// credentials to the renderer. Both outcomes are redacted: the Ok path's
    /// `stderr_tail` and the `HookFailed` message (which embeds stderr).
    #[tokio::test]
    async fn hook_test_redacts_secrets_on_both_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = "C:/models/ggml-base.en.bin".into();
        let state = override_test_state(tmp.path(), cfg).await;

        // Ok path: the command succeeds but echoes a credential to stderr.
        #[cfg(windows)]
        let ok_cmd = "cmd /c \"echo password=hunter2secret 1>&2\"";
        #[cfg(not(windows))]
        let ok_cmd = "sh -c \"echo password=hunter2secret 1>&2\"";
        let resp = handle_request(
            Request::HookTest {
                custom_command: Some(ok_cmd.to_string()),
            },
            &state,
        )
        .await;
        match resp {
            Response::Ok(v) => {
                let tail = v["stderr_tail"].as_str().unwrap_or_default();
                assert!(
                    !tail.contains("hunter2secret"),
                    "secret leaked through HookTest stderr: {tail}"
                );
                assert!(
                    tail.contains("password=<redacted>"),
                    "mask expected in stderr_tail, got: {tail}"
                );
            }
            other => panic!("expected Ok, got {other:?}"),
        }

        // Err path: a failing command's stderr rides inside the HookFailed
        // message — the same redaction must apply there.
        #[cfg(windows)]
        let fail_cmd = "cmd /c \"echo token=topsecret123 1>&2 & exit 3\"";
        #[cfg(not(windows))]
        let fail_cmd = "sh -c \"echo token=topsecret123 1>&2; exit 3\"";
        let resp = handle_request(
            Request::HookTest {
                custom_command: Some(fail_cmd.to_string()),
            },
            &state,
        )
        .await;
        match resp {
            Response::Err(e) => {
                assert!(
                    !e.message.contains("topsecret123"),
                    "secret leaked through the HookTest failure message: {}",
                    e.message
                );
                assert!(
                    e.message.contains("token=<redacted>"),
                    "mask expected in the failure message, got: {}",
                    e.message
                );
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }
}
