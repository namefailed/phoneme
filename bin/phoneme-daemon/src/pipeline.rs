//! Pipeline orchestration: transcribe → hook → done.
//!
//! Called by the queue worker per claimed payload.

use crate::app_state::AppState;
use phoneme_core::config::{Config, LlmPostProcessConfig};
use phoneme_core::error::Result;
use phoneme_core::{HookMetadata, HookPayload, HookRunner, RecordingId, RecordingStatus};
use phoneme_ipc::{DaemonEvent, PipelineStage};

/// Coalesce streamed deltas until this many chars accumulate, then flush one
/// LlmActivity event — keeps the event bus from being flooded token-by-token.
const DELTA_FLUSH_CHARS: usize = 48;
/// Cap on total response chars forwarded to the UI per stage (the full result
/// is still returned and stored; only the live "thinking" view is bounded).
const MAX_STREAMED_CHARS: usize = 16 * 1024;

/// Run one LLM stage (cleanup or summary) through the streaming path, emitting
/// `DaemonEvent::LlmActivity` so the GUI can show the exact prompt and the
/// response as it streams. Returns the final (normalized) text.
pub(crate) async fn run_llm_stage(
    state: &AppState,
    id: &RecordingId,
    stage: PipelineStage,
    provider: &dyn phoneme_core::LlmProvider,
    prompt: &str,
    text: &str,
) -> Result<String> {
    // (1) Start event carrying the verbatim prompt.
    state.events.emit(DaemonEvent::LlmActivity {
        id: id.clone(),
        stage,
        prompt: provider.exact_prompt(prompt, text),
        delta: String::new(),
        done: false,
    });

    // (2) Stream deltas, coalesced and capped.
    let mut pending = String::new();
    let mut streamed = 0usize;
    let result = {
        let mut on_delta = |d: &str| {
            if streamed >= MAX_STREAMED_CHARS {
                return;
            }
            let remaining = MAX_STREAMED_CHARS - streamed;
            let slice = if d.len() > remaining {
                let mut end = remaining;
                while end > 0 && !d.is_char_boundary(end) {
                    end -= 1;
                }
                &d[..end]
            } else {
                d
            };
            pending.push_str(slice);
            streamed += slice.len();
            if pending.len() >= DELTA_FLUSH_CHARS {
                state.events.emit(DaemonEvent::LlmActivity {
                    id: id.clone(),
                    stage,
                    prompt: String::new(),
                    delta: std::mem::take(&mut pending),
                    done: false,
                });
            }
        };
        provider.process_streaming(prompt, text, &mut on_delta).await
    };

    // (3) Flush any tail and the terminal `done` marker (regardless of outcome).
    if !pending.is_empty() {
        state.events.emit(DaemonEvent::LlmActivity {
            id: id.clone(),
            stage,
            prompt: String::new(),
            delta: std::mem::take(&mut pending),
            done: false,
        });
    }
    state.events.emit(DaemonEvent::LlmActivity {
        id: id.clone(),
        stage,
        prompt: String::new(),
        delta: String::new(),
        done: true,
    });

    result
}
use std::time::Duration;

/// Build the effective LLM config for summaries: start from `[llm_post_process]`
/// and overlay any summary-specific provider / URL / key / model the user set.
/// Each blank summary field inherits the cleanup value, so summaries can run on
/// a fully independent provider+model or simply reuse the cleanup connection.
/// Always `enabled` — summaries have their own on/off gate (`summary.auto` /
/// the explicit on-demand request).
pub fn summary_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    let mut llm = cfg.llm_post_process.clone();
    llm.enabled = true;
    let s = &cfg.summary;
    if !s.provider.trim().is_empty() {
        llm.provider = s.provider.clone();
    }
    if !s.api_url.trim().is_empty() {
        llm.api_url = s.api_url.clone();
    }
    let key = s.api_key_str();
    if !key.trim().is_empty() {
        llm.set_api_key(key.to_string());
    }
    if !s.model.trim().is_empty() {
        llm.model = s.model.clone();
    }
    llm
}

/// Generate an LLM summary of `transcript`, returning `(summary, model)` on
/// success or `None` when summarization can't run (no usable provider) or
/// fails. Non-fatal: callers log and continue.
///
/// Summaries reuse the `[llm_post_process]` provider connection (endpoint, API
/// key, provider type); only the model and prompt are summary-specific. The
/// post-processor's `enabled` flag is irrelevant here — summarization is gated
/// by its own switch — so we force a working config clone with the summary
/// model/prompt swapped in.
pub async fn generate_summary(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
) -> Option<(String, String)> {
    if transcript.trim().is_empty() {
        return None;
    }
    let llm_cfg = summary_llm_config(cfg);
    let model = llm_cfg.model.clone();
    let llm = match state.llm.provider(&llm_cfg) {
        Some(llm) => llm,
        None => {
            tracing::warn!(
                provider = %llm_cfg.provider,
                "summary requested but no usable LLM provider is configured"
            );
            return None;
        }
    };
    match run_llm_stage(state, id, PipelineStage::Summarizing, &*llm, &cfg.summary.prompt, transcript).await {
        Ok(summary) if !summary.trim().is_empty() => Some((summary, model)),
        Ok(_) => {
            tracing::warn!("summary LLM returned empty output");
            None
        }
        Err(e) => {
            tracing::error!(error = %e, "summary generation failed");
            None
        }
    }
}

/// Generate a summary (if `summary.auto`) and persist it. Runs as the final
/// pipeline step, after post-processing/cleanup, so it summarizes the text the
/// user will actually see.
async fn maybe_auto_summarize(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
) {
    if !cfg.summary.auto {
        return;
    }
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Summarizing,
    });
    match generate_summary(state, cfg, id, transcript).await {
        Some((summary, model)) => {
            if let Err(e) = state.catalog.update_summary(id, &summary, Some(&model)).await {
                tracing::warn!(error = %e, "failed to persist auto summary");
                state.events.emit(DaemonEvent::SummaryFailed {
                    id: id.clone(),
                    error: e.to_string(),
                });
            } else {
                tracing::info!(id = %id.as_str(), "auto summary saved");
                state.events.emit(DaemonEvent::SummaryUpdated { id: id.clone() });
            }
        }
        None => {
            // Auto-summary failed — surface it distinctly (the transcript itself
            // is fine; only the optional summary step failed).
            state.events.emit(DaemonEvent::SummaryFailed {
                id: id.clone(),
                error: "summary generation failed (check the AI provider)".into(),
            });
        }
    }
}

/// Finalize an in-flight item canceled by the user: move the inbox file out of
/// `processing/`, mark the recording terminal, and emit the cancel events.
/// Best-effort — logs (but doesn't propagate) errors so a cancel always settles.
/// (Uses TranscribeFailed for parity with the pending-queue cancel until a
/// dedicated Canceled status lands.)
async fn finalize_canceled(state: &AppState, id: &RecordingId) {
    if let Err(e) = state
        .catalog
        .update_status(id, RecordingStatus::TranscribeFailed)
        .await
    {
        tracing::warn!(error = %e, "cancel: failed to set status");
    }
    if let Err(e) = state
        .inbox
        .finish_failed(id, "canceled", "canceled by user")
        .await
    {
        tracing::warn!(error = %e, "cancel: failed to move inbox item out of processing");
    }
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Failed,
    });
    state
        .events
        .emit(DaemonEvent::RecordingCancelled { id: id.clone() });
    tracing::info!(id = %id, "in-flight recording canceled by user");
}

/// Process a single claimed payload through the full pipeline.
///
/// Updates catalog, fires events, moves inbox files to done/ or failed/.
pub async fn run(
    state: &AppState,
    mut payload: HookPayload,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<()> {
    let id = payload.id.clone();
    state
        .events
        .emit(DaemonEvent::TranscriptionStarted { id: id.clone() });
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Transcribing,
    });

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

    // Report transcription to the unified AI-activity ("brain") popout via the
    // Transcribing stage of LlmActivity: a start event naming the model/file,
    // then a done event with timing + size once it finishes. This lets the same
    // popout that shows cleanup/summary also surface what the STT engine is up to.
    let model_label = {
        use phoneme_core::config::TranscriptionBackend as TB;
        match cfg.whisper.provider {
            TB::Local => std::path::Path::new(&cfg.whisper.model_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("local model")
                .to_string(),
            _ => cfg.whisper.model.clone(),
        }
    };
    let provider_label = format!("{:?}", cfg.whisper.provider).to_lowercase();
    let file_label = audio_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    state.events.emit(DaemonEvent::LlmActivity {
        id: id.clone(),
        stage: PipelineStage::Transcribing,
        prompt: format!(
            "Transcribing with {provider_label} · {model_label}\nfile: {file_label}\nlanguage: {}",
            language.as_deref().unwrap_or("auto-detect")
        ),
        delta: String::new(),
        done: false,
    });
    let transcribe_started = std::time::Instant::now();

    // Race transcription against cancellation. Dropping the transcribe future on
    // cancel tears down the in-flight HTTP request (reqwest futures are
    // cancel-safe); the native path stops at the next stage boundary.
    let transcript = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            state.events.emit(DaemonEvent::LlmActivity {
                id: id.clone(),
                stage: PipelineStage::Transcribing,
                prompt: String::new(),
                delta: "✕ canceled".into(),
                done: true,
            });
            finalize_canceled(state, &id).await;
            return Ok(());
        }
        res = provider.transcribe(&audio_path, language.as_deref()) => match res {
            Ok(t) => t,
            Err(e) => {
                state.events.emit(DaemonEvent::LlmActivity {
                    id: id.clone(),
                    stage: PipelineStage::Transcribing,
                    prompt: String::new(),
                    delta: format!("✕ failed: {e}"),
                    done: true,
                });
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
        }
    };

    // Checkpoint between transcription and post-processing.
    if cancel.is_cancelled() {
        finalize_canceled(state, &id).await;
        return Ok(());
    }

    // Finish the Transcribing activity entry with timing + size for the popout.
    {
        let secs = transcribe_started.elapsed().as_secs_f32();
        let chars = transcript.chars().count();
        let words = transcript.split_whitespace().count();
        state.events.emit(DaemonEvent::LlmActivity {
            id: id.clone(),
            stage: PipelineStage::Transcribing,
            prompt: String::new(),
            delta: format!("✓ {words} words · {chars} chars in {secs:.1}s"),
            done: true,
        });
    }

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
        state.events.emit(DaemonEvent::PipelineStageChanged {
            id: id.clone(),
            stage: PipelineStage::CleaningUp,
        });
        let cleanup_result = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                finalize_canceled(state, &id).await;
                return Ok(());
            }
            r = run_llm_stage(
                state,
                &id,
                PipelineStage::CleaningUp,
                &*llm,
                &cfg.llm_post_process.prompt,
                &transcript,
            ) => r,
        };
        match cleanup_result {
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

    // Record which post-processing model was used and whether diarization was
    // actually applied. Diarization is only meaningfully "applied" when it
    // produced multi-speaker labels — both the local diarizer (`assign_speakers`)
    // and the cloud providers emit a "[Speaker N]: " prefix and fall back to
    // plain, unlabeled text when diarization is off, fails, or finds ≤1 speaker.
    // So a recording is diarized iff its raw (pre-cleanup) transcript carries
    // those labels — not merely because the setting is enabled. (Check the raw
    // transcript: LLM cleanup may strip the labels from the live version.)
    let diarized = raw_transcript.contains("[Speaker ");
    state
        .catalog
        .update_processing_meta(&id, cleanup_model.as_deref(), diarized)
        .await?;

    let recording = state.catalog.get(&id).await?;
    if let Some(rec) = recording {
        if rec.in_place && !transcript.is_empty() {
            tracing::info!(id = %id.as_str(), "in-place dictation: typing transcript at cursor");
            // Surface failures instead of `unwrap()`-panicking the worker or
            // silently dropping the typing result (the previous behavior, which
            // made a failed in-place dictation look like nothing happened with
            // no clue why). enigo can fail to initialize (e.g. no interactive
            // input session) or to send input; we log either case at error
            // level so the cause is visible in the daemon log. Typing is
            // best-effort — a failure never fails the recording.
            match enigo::Enigo::new(&enigo::Settings::default()) {
                Ok(mut enigo) => {
                    use enigo::Keyboard;
                    if let Err(e) = enigo.text(&transcript) {
                        tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to type transcript");
                    }
                }
                Err(e) => {
                    tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to initialize input simulator");
                }
            }
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
        // Auto-summary is the final step — runs after post-processing so it
        // summarizes the text the user actually sees.
        maybe_auto_summarize(state, &cfg, &id, &transcript).await;
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
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::RunningHook,
    });
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

    // Auto-summary is the final step — runs after post-processing and hooks so
    // it summarizes the text the user actually sees.
    maybe_auto_summarize(state, &cfg, &id, &payload.transcript).await;

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
