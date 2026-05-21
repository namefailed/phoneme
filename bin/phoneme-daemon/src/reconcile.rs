//! Startup reconciliation — recover from previous crashes.
//!
//! Per spec:
//! 1. Scan inbox/processing/ → move back to pending/ (Plan 1's recover_orphans).
//! 2. Sweep catalog rows in non-terminal status with no matching inbox → mark failed.
//! 3. Log warnings for orphan WAVs (no catalog row).

use crate::app_state::AppState;

pub async fn run(state: &AppState) -> anyhow::Result<()> {
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
