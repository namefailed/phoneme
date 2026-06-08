//! Pipeline orchestration: transcribe → hook → done.
//!
//! Called by the queue worker per claimed payload.

use crate::app_state::AppState;
use phoneme_core::error::Result;
use phoneme_core::{HookMetadata, HookPayload, HookRunner, RecordingStatus};
use phoneme_ipc::DaemonEvent;
use std::time::Duration;

/// Process a single claimed payload through the full pipeline.
///
/// Updates catalog, fires events, moves inbox files to done/ or failed/.
pub async fn run(state: &AppState, mut payload: HookPayload) -> Result<()> {
    let id = payload.id.clone();
    state
        .events
        .emit(DaemonEvent::TranscriptionStarted { id: id.clone() });

    // Transcribe — reuse the process-wide client (AppState) so the HTTP
    // connection pool to the local whisper-server stays warm across items.
    let cfg = state.config.load();
    let audio_path = std::path::Path::new(&payload.audio_path).to_path_buf();
    // Filter empty string to None — frontend sends "" for "auto-detect"
    let language = cfg.whisper.language.clone().filter(|s| !s.is_empty());
    let provider = state.transcription.provider(&cfg.whisper, &cfg.diarization);
    // Hold the whisper-server permit for the whole final transcription so the
    // streaming preview backs off and can't starve it (the "Whisper timed out
    // after 60s" bug). Acquiring waits for any in-flight preview tick to finish.
    let _whisper_permit = state.whisper_sem.acquire().await;
    let transcript = match provider.transcribe(&audio_path, language.as_deref()).await {
        Ok(t) => t,
        Err(e) => {
            state
                .catalog
                .update_status(&id, RecordingStatus::TranscribeFailed)
                .await?;
            state
                .inbox
                .finish_failed(&id, "whisper_error", &e.to_string())
                .await?;
            state.events.emit(DaemonEvent::TranscriptionFailed {
                id: id.clone(),
                error: e.to_string(),
            });
            return Err(e);
        }
    };

    // Release the whisper-server permit now that transcription is done — LLM
    // post-processing and hooks below don't touch the server, so the preview
    // can resume immediately.
    drop(_whisper_permit);

    // Preserve the raw Whisper output as the "original" transcript regardless
    // of whether LLM post-processing rewrites the live version. Users can
    // always restore to this via "View original transcript" in the detail pane.
    let raw_transcript = transcript.clone();

    // Optional LLM post-processing. Non-fatal: on any failure we keep the raw
    // transcript. `provider()` returns None when disabled or provider = none.
    let mut transcript = transcript;
    let mut cleanup_model: Option<String> = None;
    if let Some(llm) = state.llm.provider(&cfg.llm_post_process) {
        match llm.process(&cfg.llm_post_process.prompt, &transcript).await {
            Ok(processed) => {
                tracing::info!("LLM post-processing succeeded");
                transcript = processed;
                cleanup_model = Some(cfg.llm_post_process.model.clone());
            }
            Err(e) => {
                tracing::error!(error = %e, "LLM post-processing failed, falling back to raw transcript");
            }
        }
    }

    payload.transcript = transcript.clone();
    // The whisper-server supervisor (Task 12) will publish the actually-loaded
    // model name; until then, fall back to the configured model_path's file
    // stem or "unknown".
    payload.model = std::path::Path::new(&cfg.whisper.model_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // `transcript` = LLM-processed (or raw if LLM is disabled/failed).
    // `raw_transcript` = raw Whisper output, always preserved as the original.
    state
        .catalog
        .update_transcript(&id, &transcript, &raw_transcript, &payload.model)
        .await?;

    // Record which post-processing model was used and whether diarization was applied
    let diarized = cfg.diarization.provider != phoneme_core::config::DiarizationBackend::None;
    state
        .catalog
        .update_processing_meta(&id, cleanup_model.as_deref(), diarized)
        .await?;

    let recording = state.catalog.get(&id).await?;
    if let Some(rec) = recording {
        if rec.in_place && !transcript.is_empty() {
            tracing::info!(
                "In-place transcription enabled, typing transcript for {}",
                id.as_str()
            );
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).unwrap();
            use enigo::Keyboard;
            let _ = enigo.text(&transcript);
        }
    }

    let embedder_guard = state.embedder.read().await;
    if let Some(embedder) = embedder_guard.as_ref() {
        match embedder.embed(&transcript) {
            Ok(vec) => {
                if let Err(e) = state.catalog.upsert_embedding(&id, &vec).await {
                    tracing::warn!(error = %e, "Failed to save embedding to catalog");
                } else {
                    tracing::info!("Saved semantic embedding for {}", id.as_str());
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to embed transcript");
            }
        }
    }
    drop(embedder_guard);

    // Hooks are optional. When `run_on_transcribe` is off, finalize the
    // recording right after transcription without firing hooks or the webhook;
    // the user can run them on demand later via "Re-fire hook". This is what
    // lets a re-transcription update the text without re-triggering side effects
    // (e.g. re-appending to an Obsidian daily note).
    if !cfg.hook.run_on_transcribe {
        state
            .catalog
            .update_status(&id, RecordingStatus::Done)
            .await?;
        state.events.emit(DaemonEvent::TranscriptionDone {
            id: id.clone(),
            transcript: transcript.clone(),
        });
        state.inbox.finish_done(&id, &payload).await?;
        return Ok(());
    }

    state
        .catalog
        .update_status(&id, RecordingStatus::HookRunning)
        .await?;
    state.events.emit(DaemonEvent::TranscriptionDone {
        id: id.clone(),
        transcript: transcript.clone(),
    });

    // Hooks.
    state
        .events
        .emit(DaemonEvent::HookStarted { id: id.clone() });
    payload.metadata = HookMetadata::current();

    let mut final_exit_code = 0;
    let mut total_duration = 0;
    let mut last_cmd = String::new();

    let expanded_cfg = cfg.expanded().unwrap_or_else(|_| (**cfg).clone());

    for cmd in &expanded_cfg.hook.commands {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            continue;
        }
        let runner = HookRunner::new(
            trimmed.to_string(),
            Duration::from_secs(cfg.hook.timeout_secs),
        );
        match runner.run(&payload).await {
            Ok(result) => {
                final_exit_code = result.exit_code;
                total_duration += result.duration_ms;
                last_cmd = cmd.clone();
                if result.exit_code != 0 {
                    break;
                }
            }
            Err(e) => {
                state
                    .catalog
                    .update_status(&id, RecordingStatus::HookFailed)
                    .await?;
                state
                    .inbox
                    .finish_failed(&id, "hook_failed", &e.to_string())
                    .await?;
                state.events.emit(DaemonEvent::HookFailed {
                    id,
                    error: e.to_string(),
                });
                return Err(e);
            }
        }
    }

    // Conditional keyword-triggered hooks: run each rule whose pattern matches
    // the (post-processed) transcript. These are supplementary — a failure is
    // logged but does NOT fail the recording, since the always-on commands
    // above already succeeded.
    for rule in &expanded_cfg.hook.keyword_rules {
        if !rule.matches(&payload.transcript) {
            continue;
        }
        let cmd = rule.command.trim();
        if cmd.is_empty() {
            continue;
        }
        let runner = HookRunner::new(cmd.to_string(), Duration::from_secs(cfg.hook.timeout_secs));
        match runner.run(&payload).await {
            Ok(result) => {
                total_duration += result.duration_ms;
                last_cmd = rule.command.clone();
                if result.exit_code != 0 {
                    tracing::warn!(pattern = %rule.pattern, exit_code = result.exit_code, "keyword hook exited non-zero");
                } else {
                    tracing::info!(pattern = %rule.pattern, "keyword hook ran");
                }
            }
            Err(e) => {
                tracing::warn!(pattern = %rule.pattern, error = %e, "keyword hook failed to run");
            }
        }
    }

    state
        .catalog
        .update_hook_result(&id, &last_cmd, final_exit_code, total_duration)
        .await?;
    state
        .catalog
        .update_status(&id, RecordingStatus::Done)
        .await?;
    state.inbox.finish_done(&id, &payload).await?;
    state.events.emit(DaemonEvent::HookDone {
        id,
        exit_code: final_exit_code,
    });

    if let Some(url) = &cfg.hook.webhook_url {
        if let Err(e) = state
            .webhook
            .post(url, Duration::from_secs(cfg.hook.timeout_secs), &payload)
            .await
        {
            tracing::warn!(error = %e, "webhook failed");
        }
    }

    Ok(())
}
