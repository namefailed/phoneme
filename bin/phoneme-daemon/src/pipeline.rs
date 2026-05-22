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
    let transcript = match state.transcription.transcribe(&cfg.whisper.external_url, Duration::from_secs(cfg.whisper.timeout_secs), &audio_path).await {
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

    payload.transcript = transcript.clone();
    // The whisper-server supervisor (Task 12) will publish the actually-loaded
    // model name; until then, fall back to the configured model_path's file
    // stem or "unknown".
    payload.model = std::path::Path::new(&cfg.whisper.model_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    state
        .catalog
        .update_transcript(&id, &transcript, &payload.model)
        .await?;
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
        let runner = HookRunner::new(trimmed.to_string(), Duration::from_secs(cfg.hook.timeout_secs));
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
        if let Err(e) = state.webhook.post(url, Duration::from_secs(cfg.hook.timeout_secs), &payload).await {
            tracing::warn!(error = %e, "webhook failed");
        }
    }

    Ok(())
}
