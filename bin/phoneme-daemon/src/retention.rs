//! Periodic retention cleanup — deletes old recordings per the configured policy.
//!
//! Runs once at startup, then again every hour while the daemon is alive.
//! Only terminal-state recordings (done / failed) are eligible; in-progress
//! recordings are never touched.

use crate::app_state::AppState;
use tokio::sync::watch;
use tokio::time::{interval, Duration, MissedTickBehavior};

pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = interval(Duration::from_secs(3600));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                run_once(&state).await;
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

async fn run_once(state: &AppState) {
    let cfg = state.config.load();
    let retention = &cfg.retention;

    if retention.max_age_days.is_none() && retention.max_count.is_none() {
        return;
    }

    match state.catalog.apply_retention(retention).await {
        Ok(paths) if paths.is_empty() => {}
        Ok(paths) => {
            tracing::info!(count = paths.len(), "retention cleanup removed recordings");
            for path in &paths {
                if let Err(e) = tokio::fs::remove_file(path).await {
                    tracing::warn!(path = %path, error = %e, "could not remove audio file");
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "retention cleanup failed");
        }
    }
}
