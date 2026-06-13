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
//!    `paused`, `transcribing`, `hook_running`) with NO matching inbox entry can
//!    never finish — mark them `transcribe_failed` so the UI shows a re-runnable
//!    failure rather than a forever-spinner. `paused` is swept too: a daemon that
//!    crashed while a recording was paused leaves no live recorder and no inbox
//!    file, so the row would otherwise spin forever.

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
        RecordingStatus::Paused,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use phoneme_core::id::RecordingId;
    use phoneme_core::types::{Recording, RecordingStatus};
    use phoneme_core::Config;

    async fn test_state(tmp: &std::path::Path) -> AppState {
        std::env::set_var("PHONEME_DATA_LOCAL", tmp.join("data"));
        AppState::new(Config::default())
            .await
            .expect("build test AppState")
    }

    fn paused_row(id: RecordingId) -> Recording {
        Recording {
            id,
            started_at: chrono::Local::now(),
            duration_ms: 0,
            audio_path: String::new(),
            transcript: None,
            model: None,
            status: RecordingStatus::Paused,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
        }
    }

    /// A row left `Paused` by a daemon that crashed mid-pause has no live
    /// recorder and no inbox file, so the sweep must flip it to
    /// `TranscribeFailed` rather than leave it spinning forever.
    #[tokio::test]
    async fn sweep_marks_orphaned_paused_row_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        let id = RecordingId::new();
        state.catalog.insert(&paused_row(id.clone())).await.unwrap();

        let swept = sweep_stale_catalog_rows(&state).await.unwrap();
        assert_eq!(swept, 1, "the orphaned paused row must be swept");

        let row = state.catalog.get(&id).await.unwrap().expect("row exists");
        assert_eq!(row.status, RecordingStatus::TranscribeFailed);
    }
}
