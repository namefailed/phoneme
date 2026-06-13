//! Queue worker — the link between the durable inbox and the pipeline. One
//! task, claiming strictly one item at a time, so transcription is serial by
//! construction (the bundled whisper-server handles one request well, many
//! poorly).
//!
//! Loop: respect the user's pause flag → `inbox.claim_next()` → publish the
//! in-flight id + a fresh cancellation token in `state.processing` (what
//! `CancelProcessing` cancels) → `pipeline::run` → clear the slot → emit
//! `QueueDepthChanged`.
//!
//! Failure policy owned here:
//! - **Transient** STT failures (unreachable / timeout) requeue the SAME
//!   item and retry with exponential backoff (30 s → 5 min), emitting
//!   `WhisperStatusChanged { reachable: false }`; after
//!   `MAX_TRANSIENT_ATTEMPTS` consecutive misses the item is declared failed
//!   so a permanently dead server can't loop one recording forever.
//! - **Permanent** pipeline errors are already quarantined by the pipeline;
//!   the worker just logs and moves on.
//! - An inbox **claim error** (antivirus lock, NTFS hiccup) retries with the
//!   same backoff rather than killing the worker.
//!
//! After every run the worker re-reads the config from disk — but only when
//! the file's mtime actually changed (a stat per run instead of a TOML
//! parse), invalidating the cached diarizer when `[diarization]` changed.
//! This is the second config-apply point next to the `ReloadConfig` IPC.

use crate::app_state::AppState;
use crate::pipeline;
use phoneme_core::Error;
use phoneme_ipc::DaemonEvent;
use std::time::{Duration, SystemTime};
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
    // Last-seen mtime of the config file. Config is only re-parsed from disk
    // when this changes — a stat() is orders of magnitude cheaper than a full
    // TOML parse + validation on every pipeline run (the common hot path).
    let mut config_mtime: Option<SystemTime> = None;

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

                // Reload config from disk only when the file was modified since
                // the last check. A stat() is far cheaper than a full TOML parse
                // + validation on every pipeline run, and the common case (no
                // change) does zero work. This preserves both invalidation points:
                // the ReloadConfig IPC (instant, triggered by the user) and this
                // post-run path (catches background edits between IPC calls).
                let current_mtime = phoneme_core::config::resolved_config_path()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .and_then(|m| m.modified().ok());
                let changed = match (config_mtime, current_mtime) {
                    (Some(prev), Some(cur)) => cur != prev,
                    // No previous mtime recorded yet — treat as changed so the
                    // first post-run pass always validates the live config.
                    (None, _) => true,
                    // Config file missing (e.g. default in-memory config); skip.
                    (_, None) => false,
                };
                if changed {
                    config_mtime = current_mtime;
                    match crate::load_config() {
                        Ok(cfg) => {
                            // Config changed on disk: drop the cached local
                            // diarization pipeline if `[diarization]` changed
                            // (backend switch / model path). One of the two daemon
                            // invalidation points — the other is the ReloadConfig
                            // IPC handler. A same-config reload never reaches here.
                            state
                                .transcription
                                .diarizer_cache()
                                .invalidate_if_stale(&cfg.diarization);
                            state.config.store(std::sync::Arc::new(cfg));
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "failed to reload config after pipeline run");
                        }
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
