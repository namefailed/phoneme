//! Periodic retention cleanup — deletes old recordings per the configured
//! `[retention]` policy (`max_age_days` / `max_count`; both unset = the
//! task is a no-op).
//!
//! A background loop spawned from `main`, ticking hourly until shutdown.
//! Each pass first emits a [`phoneme_ipc::DaemonEvent::RetentionWarning`]
//! (at most once per 24 h) for recordings entering the next 24 h deletion
//! window — the UI's chance to warn before audio disappears — then asks the
//! catalog to apply the policy and unlinks the returned WAV paths
//! best-effort. Only terminal-state recordings (done / failed) are
//! eligible; in-progress recordings are never touched.

use crate::app_state::AppState;
use phoneme_ipc::DaemonEvent;
use std::time::Instant;
use tokio::sync::watch;
use tokio::time::{interval, Duration, MissedTickBehavior};

pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = interval(Duration::from_secs(3600));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_warning: Option<Instant> = None;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                run_once(&state, &mut last_warning).await;
            }
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    tracing::debug!("retention task: shutdown received");
                    return;
                }
            }
        }
    }
}

async fn run_once(state: &AppState, last_warning: &mut Option<Instant>) {
    let cfg = state.config.load();
    let retention = &cfg.retention;

    if retention.max_age_days.is_none() && retention.max_count.is_none() {
        return;
    }

    // Pre-deletion warning for 24h age boundary
    if let Ok(count) = state
        .catalog
        .analyze_upcoming_retention(retention, 24)
        .await
    {
        if count > 0 {
            let should_warn = !matches!(last_warning, Some(t) if t.elapsed().as_secs() < 86400);
            if should_warn {
                state
                    .events
                    .emit(DaemonEvent::RetentionWarning { count, hours: 24 });
                *last_warning = Some(Instant::now());
                tracing::info!(count, "emitted retention warning");
            }
        }
    }

    match state.catalog.apply_retention(retention).await {
        Ok(paths) if paths.is_empty() => {}
        Ok(paths) => {
            tracing::info!(count = paths.len(), "retention cleanup removed recordings");
            for path in &paths {
                if let Err(e) = tokio::fs::remove_file(path).await {
                    // A WAV already gone (manual delete, an earlier partial sweep,
                    // or an ephemeral dictation) is the expected steady state for a
                    // pruned row — not worth a warning. Anything else (permissions,
                    // a locked file) still surfaces.
                    if e.kind() == std::io::ErrorKind::NotFound {
                        tracing::debug!(path = %path, "audio file already gone; nothing to remove");
                    } else {
                        tracing::warn!(path = %path, error = %e, "could not remove audio file");
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "retention cleanup failed");
        }
    }
}
