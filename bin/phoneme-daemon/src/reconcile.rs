//! Startup reconciliation — make the on-disk world consistent before any new
//! work runs. Called once from `main`, before the queue worker spawns, so the
//! worker only ever sees a sane inbox.
//!
//! A daemon can die mid-pipeline (crash, kill, power loss); the durable inbox
//! is what makes that survivable, and this module is the recovery half:
//! 1. Re-run the first-run hook copy ([`crate::first_run`], idempotent).
//! 2. Inbox sweep: anything stranded in `processing/` (claimed when the old
//!    daemon died) moves back to `pending/`, so the recording transcribes on
//!    this run instead of being lost.
//! 3. Catalog sweep: rows stuck in an in-progress status (`recording`,
//!    `transcribing`, `hook_running`) with NO matching inbox entry can never
//!    finish — mark them `transcribe_failed` so the UI shows a re-runnable
//!    failure rather than a forever-spinner.

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
