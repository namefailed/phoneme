//! Queue worker — drains inbox/pending serially.
//!
//! Loop: claim_next → pipeline::run → emit QueueDepthChanged. On
//! transient failure (WhisperUnreachable, WhisperTimeout) sleep with exponential
//! backoff and retry.

use crate::app_state::AppState;
use crate::pipeline;
use phoneme_core::Error;
use phoneme_ipc::DaemonEvent;
use std::time::Duration;
use tokio::sync::watch;

pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
    let mut backoff = Duration::from_secs(30);
    let max_backoff = Duration::from_secs(300);

    loop {
        if *shutdown.borrow() {
            tracing::info!("queue worker shutting down");
            return Ok(());
        }

        // When the user has paused the queue, don't claim new work — just idle
        // and poll. The currently-processing item (already claimed) is never
        // interrupted; only the next claim is gated.
        if state.inbox.is_paused().await {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                _ = shutdown.changed() => return Ok(()),
            }
            continue;
        }

        // A transient I/O error (antivirus lock, NTFS journal flush) must not
        // permanently kill the worker — retry with the same backoff the Whisper
        // path uses. `?` here would silently stop all transcription.
        let claimed = match state.inbox.claim_next().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, ?backoff, "inbox claim failed; retrying");
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = shutdown.changed() => return Ok(()),
                }
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };
        match claimed {
            Some(payload) => {
                emit_queue_depth(&state).await;
                // Publish a fresh cancellation token for this in-flight item so
                // `CancelProcessing` can abort the whisper/LLM work mid-flight.
                let token = tokio_util::sync::CancellationToken::new();
                if let Ok(mut slot) = state.processing.lock() {
                    *slot = Some((payload.id.clone(), token.clone()));
                }
                let result = pipeline::run(&state, payload, token).await;
                if let Ok(mut slot) = state.processing.lock() {
                    *slot = None;
                }
                match result {
                    Ok(()) => {
                        backoff = Duration::from_secs(30); // reset on success
                    }
                    Err(Error::WhisperUnreachable { .. }) | Err(Error::WhisperTimeout { .. }) => {
                        state
                            .events
                            .emit(DaemonEvent::WhisperStatusChanged { reachable: false });
                        tracing::warn!(?backoff, "Whisper unreachable; sleeping before retry");
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = shutdown.changed() => return Ok(()),
                        }
                        backoff = (backoff * 2).min(max_backoff);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "fatal pipeline error (failed)");
                    }
                }

                // Restore any temporarily overridden config settings layered in
                // by a re-run (one-time hook toggle / cleanup / summary overrides
                // from RetranscribeRecording). NOTE: this no longer touches the
                // whisper MODEL — a model override is now applied per-job in the
                // pipeline via `whisper_model_override`, never via the global
                // config, so reloading here can't trigger a whisper-server restart
                // (the double-restart that thrashed the server, #49). It only
                // reverts the server-independent temp settings.
                match crate::load_config() {
                    Ok(cfg) => {
                        state.config.store(std::sync::Arc::new(cfg));
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to reload config after pipeline run");
                    }
                }
                emit_queue_depth(&state).await;
            }
            None => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    _ = shutdown.changed() => return Ok(()),
                }
            }
        }
    }
}

pub async fn emit_queue_depth(state: &AppState) {
    if let Ok(counts) = state.inbox.counts().await {
        state.events.emit(DaemonEvent::QueueDepthChanged {
            pending: counts.pending,
            processing: counts.processing,
            failed: counts.failed,
        });
    }
}

#[cfg(test)]
#[path = "queue_worker_test.rs"]
mod tests;
