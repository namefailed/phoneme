//! Startup reconciliation — recover from previous crashes.
//!
//! Startup recovery operations for the daemon.
//!
//! Responsibilities:
//! 1. Scan inbox/processing/ → move back to pending/ to recover stranded recordings.
//! 2. Scan catalog where status=processing → set status=pending to retry transcription.

use crate::app_state::AppState;
use crate::first_run;

pub async fn run(state: &AppState) -> anyhow::Result<()> {
    // First-run: copy reference hooks into user's config dir.
    if let Err(e) = first_run::ensure_hooks_copied(state).await {
        tracing::warn!(error = %e, "first-run hook copy failed");
    }

    // Step 1: requeue inbox orphans.
    let orphans = state.inbox.recover_orphans().await?;
    if !orphans.is_empty() {
        tracing::warn!(count = orphans.len(), "recovered orphan inbox entries");
    }

    // Step 2: catalog sweep.
    let stale = sweep_stale_catalog_rows(state).await?;
    if stale > 0 {
        tracing::warn!(count = stale, "marked stale catalog rows as failed");
    }

    Ok(())
}

async fn sweep_stale_catalog_rows(state: &AppState) -> anyhow::Result<usize> {
    use phoneme_core::{ListFilter, RecordingStatus};

    let mut count = 0;
    for status in [
        RecordingStatus::Recording,
        RecordingStatus::Transcribing,
        RecordingStatus::HookRunning,
    ] {
        let rows = state
            .catalog
            .list(&ListFilter {
                status: Some(status),
                ..Default::default()
            })
            .await?;
        for row in rows {
            let processing_path = state
                .paths
                .inbox_dir
                .join("processing")
                .join(format!("{}.json", row.id));
            let pending_path = state
                .paths
                .inbox_dir
                .join("pending")
                .join(format!("{}.json", row.id));
            if !processing_path.exists() && !pending_path.exists() {
                let _ = state
                    .catalog
                    .update_status(&row.id, RecordingStatus::TranscribeFailed)
                    .await;
                count += 1;
            }
        }
    }
    Ok(count)
}
