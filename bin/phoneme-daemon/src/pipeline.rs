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
    // connection pool to the local llama-server stays warm across items.
    let cfg = &state.config;
    let audio_path = std::path::Path::new(&payload.audio_path).to_path_buf();
    let transcript = match state.transcription.transcribe(&audio_path).await {
        Ok(t) => t,
        Err(e) => {
            state
                .catalog
                .update_status(&id, RecordingStatus::TranscribeFailed)
                .await?;
            state
                .inbox
                .finish_failed(&id, "llm_error", &e.to_string())
                .await?;
            state.events.emit(DaemonEvent::TranscriptionFailed {
                id: id.clone(),
                error: e.to_string(),
            });
            return Err(e);
        }
    };

    payload.transcript = transcript.clone();
    // The llama-server supervisor (Task 12) will publish the actually-loaded
    // model name; until then, fall back to the configured model_path's file
    // stem or "unknown".
    payload.model = std::path::Path::new(&cfg.llm.model_path)
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

    // Hook.
    state
        .events
        .emit(DaemonEvent::HookStarted { id: id.clone() });
    let runner = HookRunner::new(
        cfg.hook.command.clone(),
        Duration::from_secs(cfg.hook.timeout_secs),
    );
    payload.metadata = HookMetadata::current();
    match runner.run(&payload).await {
        Ok(result) => {
            state
                .catalog
                .update_hook_result(&id, &cfg.hook.command, result.exit_code, result.duration_ms)
                .await?;
            state
                .catalog
                .update_status(&id, RecordingStatus::Done)
                .await?;
            state.inbox.finish_done(&id, &payload).await?;
            state.events.emit(DaemonEvent::HookDone {
                id,
                exit_code: result.exit_code,
            });

            if let Some(wh) = &state.webhook {
                if let Err(e) = wh.post(&payload).await {
                    tracing::warn!(error = %e, "webhook failed");
                }
            }

            Ok(())
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
            Err(e)
        }
    }
}
