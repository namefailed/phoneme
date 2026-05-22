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
                let counts = state.inbox.counts().await.unwrap_or_default();
                state.events.emit(DaemonEvent::QueueDepthChanged {
                    pending: counts.pending,
                    processing: counts.processing,
                    failed: counts.failed,
                });
                match pipeline::run(&state, payload).await {
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
                        tracing::warn!(error = %e, "pipeline error (non-transient)");
                    }
                }
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
