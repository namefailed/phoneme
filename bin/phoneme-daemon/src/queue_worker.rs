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

/// Give up on an item after this many consecutive TRANSIENT transcribe
/// failures (unreachable / timeout). High on purpose: with max backoff this is
/// ~25 minutes of a dead server before an item is declared failed — Doctor's
/// restart usually heals it long before. Permanent errors never retry.
const MAX_TRANSIENT_ATTEMPTS: u32 = 5;

pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
    let mut backoff = Duration::from_secs(30);
    let max_backoff = Duration::from_secs(300);
    // Consecutive transient-failure count per recording, so a persistently
    // failing item eventually lands in failed/ instead of looping forever.
    let mut attempts: std::collections::HashMap<phoneme_core::RecordingId, u32> =
        std::collections::HashMap::new();

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
                // The payload moves into the pipeline; keep the id for the
                // retry bookkeeping below.
                let rec_id = payload.id.clone();
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
                        attempts.remove(&rec_id);
                    }
                    Err(e @ Error::WhisperUnreachable { .. })
                    | Err(e @ Error::WhisperTimeout { .. }) => {
                        state
                            .events
                            .emit(DaemonEvent::WhisperStatusChanged { reachable: false });
                        // The pipeline left this transient failure CLAIMED (it
                        // never reaches failed/) — put it back in pending so the
                        // SAME recording retries after the backoff, instead of
                        // being silently lost while the worker moves on.
                        let tries = attempts.entry(rec_id.clone()).or_insert(0);
                        *tries += 1;
                        if *tries >= MAX_TRANSIENT_ATTEMPTS {
                            tracing::error!(
                                id = %rec_id.as_str(),
                                tries = *tries,
                                "giving up after repeated transient whisper failures"
                            );
                            attempts.remove(&rec_id);
                            let _ = state
                                .catalog
                                .update_status(
                                    &rec_id,
                                    phoneme_core::RecordingStatus::TranscribeFailed,
                                )
                                .await;
                            let _ = state
                                .inbox
                                .finish_failed(&rec_id, "whisper_error", &e.to_string())
                                .await;
                            state.events.emit(DaemonEvent::TranscriptionFailed {
                                id: rec_id.clone(),
                                error: format!("{e} (after {MAX_TRANSIENT_ATTEMPTS} attempts)"),
                            });
                        } else if let Err(rq) = state.inbox.requeue(&rec_id).await {
                            tracing::error!(id = %rec_id.as_str(), error = %rq, "failed to requeue after transient error");
                        }
                        tracing::warn!(?backoff, "Whisper unreachable; sleeping before retry");
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = shutdown.changed() => return Ok(()),
                        }
                        backoff = (backoff * 2).min(max_backoff);
                    }
                    Err(e) => {
                        attempts.remove(&rec_id);
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
