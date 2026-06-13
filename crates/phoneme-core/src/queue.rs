//! The inbox: a filesystem-backed work queue for the transcription pipeline.
//!
//! This module owns [`InboxQueue`], the durable hand-off between "a recording
//! finished capturing" and "the daemon's worker transcribed it". The daemon
//! enqueues a payload when a recording stops; a single worker claims items one
//! at a time and runs the pipeline against each.
//!
//! Why the filesystem instead of an in-memory queue: state must survive a crash
//! or restart. Each item is a JSON file, and every state change is an atomic
//! rename between four subdirectories — `pending/` → `processing/` →
//! `done/`/`failed/`. That gives crash recovery for free ([`recover_orphans`]
//! re-queues anything stuck in `processing/`, and is idempotent across the
//! finish-then-crash window). Two dot-files control ordering and pausing without
//! showing up as payloads: `.queue-order` (the user's custom claim order) and
//! `.queue-paused` (a sentinel the worker checks before each claim). The badge
//! counts in the GUI come from [`InboxQueue::counts`].
//!
//! [`recover_orphans`]: InboxQueue::recover_orphans

use crate::error::{Error, Result};
use crate::id::RecordingId;
use crate::types::HookPayload;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Filename of the pending-queue order manifest (a JSON array of recording ids
/// in desired claim order). Deliberately has no `.json` extension so the
/// payload scan ignores it.
const ORDER_FILE: &str = ".queue-order";

/// Sentinel marking the queue as paused. When present in the inbox root the
/// worker stops claiming new pending items (the in-flight item, if any, still
/// finishes). No `.json` extension so payload scans ignore it.
const PAUSE_FILE: &str = ".queue-paused";

/// Which directory of the inbox a payload lives in (one per subdirectory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxState {
    /// Awaiting a worker (`pending/`).
    Pending,
    /// Being transcribed/post-processed right now (`processing/`, at most one).
    Processing,
    /// Completed (`done/`).
    Done,
    /// Quarantined after an error or cancellation (`failed/`).
    Failed,
}

impl InboxState {
    /// The subdirectory name this state maps to under the inbox root.
    pub fn subdir(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Count of payloads in each inbox state (powers the queue badge).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InboxCounts {
    /// Files in `pending/`.
    pub pending: usize,
    /// Files in `processing/`.
    pub processing: usize,
    /// Files in `done/`.
    pub done: usize,
    /// Files in `failed/`.
    pub failed: usize,
}

/// The record written into `failed/` when an item is quarantined, capturing why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedPayload {
    /// The recording this failure belongs to.
    pub id: RecordingId,
    /// Short machine-readable failure category (e.g. `"corrupt_payload"`).
    pub error_kind: String,
    /// Human-readable failure detail.
    pub error_message: String,
}

/// Filesystem-backed work queue for transcribing and processing recordings.
///
/// State transitions are implemented as atomic file renames across the following
/// subdirectories (created automatically under the `root` path):
/// - `pending/`: Payload JSON files awaiting processing. Ordered chronologically
///   by filename unless overridden by the user-defined order manifest.
/// - `processing/`: The single payload currently undergoing transcription or post-processing.
/// - `done/`: Completed payloads, retained temporarily before deletion or archival.
/// - `failed/`: Payloads that failed due to an error, accompanied by error logs.
///
/// ### Metadata & Control Files:
/// - `pending/.queue-order`: A JSON array of recording IDs representing the user's
///   custom drag-and-drop sequencing. Any ID not listed here falls back to chronological.
/// - `.queue-paused`: A zero-byte sentinel file. When present, the daemon's queue worker
///   is blocked from claiming new pending tasks (the active processing task is unaffected).
#[derive(Debug, Clone)]
pub struct InboxQueue {
    root: PathBuf,
}

impl InboxQueue {
    /// Create (or open) an inbox at `root`. Creates the four subdirectories
    /// if missing.
    pub async fn new(root: &Path) -> Result<Self> {
        for state in [
            InboxState::Pending,
            InboxState::Processing,
            InboxState::Done,
            InboxState::Failed,
        ] {
            fs::create_dir_all(root.join(state.subdir())).await?;
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Atomically write a new pending payload.
    ///
    /// Implementation: write to a temp file in the same directory, then rename
    /// to the final name. Rename on the same filesystem is atomic.
    pub async fn enqueue(&self, payload: &HookPayload) -> Result<()> {
        let pending = self.root.join("pending");
        let final_path = pending.join(format!("{}.json", payload.id));
        let temp_path = pending.join(format!("{}.json.tmp", payload.id));
        let json = serde_json::to_vec_pretty(payload)?;
        fs::write(&temp_path, &json).await?;
        fs::rename(&temp_path, &final_path).await?;
        Ok(())
    }

    /// Pending payload files in effective claim order: any user-defined order
    /// (from the `.queue-order` manifest) first, then remaining files by
    /// filename (chronological). The manifest is a plain JSON array of ids; it
    /// has no `.json` extension so the payload scan ignores it.
    async fn ordered_pending(&self) -> Result<Vec<PathBuf>> {
        let pending = self.root.join("pending");
        let files = read_json_entries_sorted(&pending).await?; // chronological
        let order = self.read_order().await;
        let mut ordered: Vec<PathBuf> = Vec::new();
        let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();
        // 1. Files explicitly ordered by the user, in that order.
        for id in &order {
            if let Some(p) = files
                .iter()
                .find(|f| f.file_stem().and_then(|s| s.to_str()) == Some(id.as_str()))
            {
                ordered.push(p.clone());
                placed.insert(id.clone());
            }
        }
        // 2. Everything else (newly enqueued, never reordered) chronologically.
        for f in &files {
            if let Some(stem) = f.file_stem().and_then(|s| s.to_str()) {
                if !placed.contains(stem) {
                    ordered.push(f.clone());
                }
            }
        }
        Ok(ordered)
    }

    /// Read the `.queue-order` manifest (list of ids in desired claim order).
    async fn read_order(&self) -> Vec<String> {
        let p = self.root.join("pending").join(ORDER_FILE);
        match fs::read(&p).await {
            Ok(bytes) => serde_json::from_slice::<Vec<String>>(&bytes).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// Set the desired claim order for pending items (user drag/up-down). Ids
    /// not present are ignored when claiming; missing ids fall back to
    /// chronological order. Persisted atomically.
    pub async fn set_order(&self, ids: &[RecordingId]) -> Result<()> {
        let list: Vec<String> = ids.iter().map(|i| i.as_str().to_string()).collect();
        let json = serde_json::to_vec(&list)?;
        let dir = self.root.join("pending");
        let final_path = dir.join(ORDER_FILE);
        let temp_path = dir.join(format!("{ORDER_FILE}.tmp"));
        fs::write(&temp_path, &json).await?;
        fs::rename(&temp_path, &final_path).await?;
        Ok(())
    }

    /// Claim the next pending payload in effective order (moving it to
    /// `processing/`). Returns `None` if there's nothing pending.
    ///
    /// A corrupt (unparseable) payload is claimed exactly once, quarantined to
    /// `failed/`, and reported as `Ok(None)` so the caller simply tries the
    /// next file. The rename-before-parse ordering is what makes this work: if
    /// we parsed first, a single corrupt file at the head of the queue would
    /// fail every `claim_next()` call forever and starve every file behind it.
    pub async fn claim_next(&self) -> Result<Option<HookPayload>> {
        // Honor the user-defined order (manifest) first, then chronological.
        let entries = self.ordered_pending().await?;
        // Walk the queue oldest-first and return the first file we can actually
        // claim. Crucially, a file we *can't* claim (malformed name, OS-level
        // rename failure from an AV/dangling-handle lock, or a corrupt payload)
        // must NOT block the rest of the queue: we skip it and try the next one.
        // A locked file is left in pending/ and retried on the next poll; a
        // structurally-bad file is quarantined to failed/. The old code always
        // took entries.first() and propagated a rename error, so one stuck file
        // would starve the entire queue forever.
        for file in &entries {
            let Some(id_str) = file.file_stem().and_then(|s| s.to_str()) else {
                tracing::warn!(file = %file.display(), "skipping inbox file with non-utf8 name");
                continue;
            };
            // Reject filenames that aren't a canonical RecordingId (e.g. a file a
            // user manually dropped into pending/). Without this, the fixed-offset
            // slicing in file_stem()/day_folder() — and the debug_assert in
            // from_str_unchecked — would panic the daemon. Quarantine and move on.
            let Some(id) = RecordingId::parse(id_str) else {
                if let Some(name) = file.file_name() {
                    let _ = fs::rename(file, self.root.join("failed").join(name)).await;
                }
                tracing::warn!(file = %file.display(), "quarantined inbox file with malformed id");
                continue;
            };
            let processing = self
                .root
                .join("processing")
                .join(format!("{}.json", id.as_str()));
            // If the claim rename fails (file locked by AV, a dangling read
            // handle from a crashed process, etc.) skip this file for now and try
            // the next — never let one un-renameable file starve the queue. It
            // stays in pending/ and is retried on the next poll once unlocked.
            if let Err(e) = fs::rename(file, &processing).await {
                tracing::debug!(file = %file.display(), error = %e, "could not claim inbox file (locked?); skipping for now");
                continue;
            }
            match read_payload(&processing).await {
                Ok(p) => return Ok(Some(p)),
                Err(e) => {
                    // Quarantine the unparseable file and keep scanning.
                    self.finish_failed(&id, "corrupt_payload", &e.to_string())
                        .await?;
                    continue;
                }
            }
        }
        Ok(None)
    }

    /// Move a processing payload to `done/`, replacing it with the final form
    /// (with transcript, hook result, etc.).
    pub async fn finish_done(&self, id: &RecordingId, payload: &HookPayload) -> Result<()> {
        let processing = self.root.join("processing").join(format!("{id}.json"));
        let done = self.root.join("done").join(format!("{id}.json"));
        let json = serde_json::to_vec_pretty(payload)?;
        let temp = self.root.join("done").join(format!("{id}.json.tmp"));
        fs::write(&temp, &json).await?;
        fs::rename(&temp, &done).await?;
        if fs::try_exists(&processing).await.unwrap_or(false) {
            fs::remove_file(&processing).await?;
        }
        Ok(())
    }

    /// Archive a cancelled item's processing payload to `done/` as-is.
    ///
    /// A cancel is the user's own action, not a failure — the payload must
    /// not land in the `failed/` quarantine (the queue badge counts that
    /// directory). Moving the file unchanged into `done/` keeps crash
    /// recovery idempotent: a `processing/` file orphaned mid-cancel pairs
    /// with the done marker and is dropped instead of re-run.
    pub async fn finish_cancelled(&self, id: &RecordingId) -> Result<()> {
        let processing = self.root.join("processing").join(format!("{id}.json"));
        let done = self.root.join("done").join(format!("{id}.json"));
        if fs::try_exists(&done).await.unwrap_or(false) {
            fs::remove_file(&done).await?;
        }
        if fs::try_exists(&processing).await.unwrap_or(false) {
            fs::rename(&processing, &done).await?;
        }
        Ok(())
    }

    /// Move a processing payload to `failed/`, writing a failure record.
    pub async fn finish_failed(
        &self,
        id: &RecordingId,
        error_kind: &str,
        error_message: &str,
    ) -> Result<()> {
        let processing = self.root.join("processing").join(format!("{id}.json"));
        let failed = self.root.join("failed").join(format!("{id}.json"));
        let record = FailedPayload {
            id: id.clone(),
            error_kind: error_kind.to_string(),
            error_message: error_message.to_string(),
        };
        let json = serde_json::to_vec_pretty(&record)?;
        let temp = self.root.join("failed").join(format!("{id}.json.tmp"));
        fs::write(&temp, &json).await?;
        fs::rename(&temp, &failed).await?;
        if fs::try_exists(&processing).await.unwrap_or(false) {
            fs::remove_file(&processing).await?;
        }
        Ok(())
    }

    /// Move a processing payload back to pending (e.g., a user-initiated
    /// re-transcribe from `failed/` goes pending -> processing on next claim).
    pub async fn requeue(&self, id: &RecordingId) -> Result<()> {
        let processing = self.root.join("processing").join(format!("{id}.json"));
        let pending = self.root.join("pending").join(format!("{id}.json"));
        if fs::try_exists(&processing).await.unwrap_or(false) {
            fs::rename(&processing, &pending).await?;
        }
        Ok(())
    }

    /// Move any files left in `processing/` back to `pending/`. Returns the
    /// list of ids recovered. Called by the daemon at startup.
    ///
    /// ### Crash-recovery note
    ///
    /// [`Self::finish_done`] writes the done payload atomically and then removes the
    /// processing file. A crash in the window between those two steps leaves
    /// both `done/<id>.json` and `processing/<id>.json`. This method detects
    /// that pair and drops the stale processing file rather than re-queuing the
    /// already-complete item — making recovery idempotent across that crash
    /// window.
    pub async fn recover_orphans(&self) -> Result<Vec<RecordingId>> {
        let mut recovered = vec![];
        let mut dir = fs::read_dir(self.root.join("processing")).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let id_str = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
                Error::Internal(format!("bad orphan filename: {}", path.display()))
            })?;
            // Skip/quarantine orphans whose name isn't a valid RecordingId
            // instead of slicing out of bounds on them.
            let Some(id) = RecordingId::parse(id_str) else {
                if let Some(name) = path.file_name() {
                    let _ = fs::rename(&path, self.root.join("failed").join(name)).await;
                }
                tracing::warn!(file = %path.display(), "quarantined orphan with malformed id");
                continue;
            };
            // A done/ file for this id means finish_done completed but the
            // subsequent remove_file on the processing/ copy crashed. The work
            // is already recorded as done — drop the stale processing file
            // instead of re-queuing the job and running the pipeline again.
            let done_path = self.root.join("done").join(format!("{}.json", id.as_str()));
            if fs::try_exists(&done_path).await.unwrap_or(false) {
                tracing::info!(
                    id = %id.as_str(),
                    "recovery: dropping stale processing file (done copy already exists)"
                );
                let _ = fs::remove_file(&path).await;
                continue;
            }
            let dest = self
                .root
                .join("pending")
                .join(format!("{}.json", id.as_str()));
            fs::rename(&path, &dest).await?;
            recovered.push(id);
        }
        Ok(recovered)
    }

    /// List the payloads currently in `pending/`, oldest-first (the order they
    /// will be claimed). Unparseable files are skipped (not surfaced to the UI).
    pub async fn list_pending(&self) -> Result<Vec<HookPayload>> {
        let mut out = vec![];
        // Same effective order the worker will claim in (manifest, then chrono).
        for path in self.ordered_pending().await? {
            if let Ok(p) = read_payload(&path).await {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// List the payloads currently in `processing/` (normally at most one — the
    /// item the worker is actively transcribing).
    pub async fn list_processing(&self) -> Result<Vec<HookPayload>> {
        let mut out = vec![];
        for path in read_json_entries_sorted(&self.root.join("processing")).await? {
            if let Ok(p) = read_payload(&path).await {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Remove a still-pending payload from the queue (user-initiated cancel).
    /// Returns `true` if it was present and removed; `false` if it was already
    /// claimed/gone (so the caller can report that it couldn't be cancelled).
    pub async fn cancel_pending(&self, id: &RecordingId) -> Result<bool> {
        let path = self.root.join("pending").join(format!("{id}.json"));
        if fs::try_exists(&path).await.unwrap_or(false) {
            fs::remove_file(&path).await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Pause or resume the queue. Pausing drops a sentinel file the worker
    /// checks before each claim; resuming removes it. The currently-processing
    /// item is never interrupted — only new claims are gated.
    pub async fn set_paused(&self, paused: bool) -> Result<()> {
        let path = self.root.join(PAUSE_FILE);
        if paused {
            fs::write(&path, b"").await?;
        } else if fs::try_exists(&path).await.unwrap_or(false) {
            fs::remove_file(&path).await?;
        }
        Ok(())
    }

    /// Whether the queue is currently paused (the worker should not claim).
    pub async fn is_paused(&self) -> bool {
        fs::try_exists(self.root.join(PAUSE_FILE))
            .await
            .unwrap_or(false)
    }

    /// Remove ALL still-pending payloads from the queue (user-initiated
    /// "clear queue"). The in-flight `processing/` item is left untouched. Also
    /// clears the order manifest. Returns the ids of the removed items so the
    /// caller can mark them terminal in the catalog.
    pub async fn cancel_all_pending(&self) -> Result<Vec<RecordingId>> {
        let mut removed = Vec::new();
        for path in read_json_entries_sorted(&self.root.join("pending")).await? {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(RecordingId::parse);
            if fs::remove_file(&path).await.is_ok() {
                if let Some(id) = id {
                    removed.push(id);
                }
            }
        }
        // The manifest now references nothing; drop it so a future enqueue
        // starts clean.
        let order = self.root.join("pending").join(ORDER_FILE);
        if fs::try_exists(&order).await.unwrap_or(false) {
            let _ = fs::remove_file(&order).await;
        }
        Ok(removed)
    }

    /// Remove every payload quarantined in `failed/` (user-initiated "dismiss
    /// failed"). The `failed/` folder only ever grows — permanent transcription
    /// errors, hook failures, corrupt payloads, and user cancellations all land
    /// here and nothing else empties it — so this is the way to acknowledge and
    /// clear them. The catalog rows (with their `transcribe_failed`/`hook_failed`
    /// status) are untouched; only the inbox quarantine is cleared. Returns how
    /// many files were removed.
    pub async fn clear_failed(&self) -> Result<usize> {
        let mut removed = 0;
        for path in read_json_entries_sorted(&self.root.join("failed")).await? {
            if fs::remove_file(&path).await.is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Count files in each inbox subdirectory.
    pub async fn counts(&self) -> Result<InboxCounts> {
        Ok(InboxCounts {
            pending: count_json(&self.root.join("pending")).await?,
            processing: count_json(&self.root.join("processing")).await?,
            done: count_json(&self.root.join("done")).await?,
            failed: count_json(&self.root.join("failed")).await?,
        })
    }
}

async fn read_payload(path: &Path) -> Result<HookPayload> {
    let bytes = fs::read(path).await?;
    let payload: HookPayload = serde_json::from_slice(&bytes)?;
    Ok(payload)
}

async fn read_json_entries_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    let mut entries = fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

async fn count_json(dir: &Path) -> Result<usize> {
    let mut count = 0;
    let mut entries = fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::RecordingId;
    use crate::types::{HookMetadata, HookPayload};

    fn make_payload() -> HookPayload {
        HookPayload {
            id: RecordingId::new(),
            timestamp: chrono::Local::now(),
            transcript: "test transcript".to_string(),
            audio_path: "test.wav".into(),
            duration_ms: 5000,
            model: "test-model".into(),
            metadata: HookMetadata::current(),
        }
    }

    async fn open_inbox(dir: &std::path::Path) -> InboxQueue {
        InboxQueue::new(dir).await.expect("open inbox")
    }

    // -------------------------------------------------------------------------
    // Basic lifecycle
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn enqueue_and_claim() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();
        inbox.enqueue(&p).await.unwrap();

        let counts = inbox.counts().await.unwrap();
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.processing, 0);

        let claimed = inbox.claim_next().await.unwrap();
        assert!(claimed.is_some());
        let claimed = claimed.unwrap();
        assert_eq!(claimed.id, p.id);

        let counts = inbox.counts().await.unwrap();
        assert_eq!(counts.pending, 0);
        assert_eq!(counts.processing, 1);
    }

    #[tokio::test]
    async fn finish_done_moves_to_done_and_removes_processing() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();
        inbox.enqueue(&p).await.unwrap();
        inbox.claim_next().await.unwrap();

        inbox.finish_done(&p.id, &p).await.unwrap();

        let counts = inbox.counts().await.unwrap();
        assert_eq!(counts.processing, 0, "processing file must be removed");
        assert_eq!(counts.done, 1, "done file must exist");

        // No stale processing file.
        let proc = tmp
            .path()
            .join("processing")
            .join(format!("{}.json", p.id.as_str()));
        assert!(
            !proc.exists(),
            "processing file must not survive finish_done"
        );
    }

    #[tokio::test]
    async fn finish_failed_moves_to_failed_and_removes_processing() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();
        inbox.enqueue(&p).await.unwrap();
        inbox.claim_next().await.unwrap();

        inbox
            .finish_failed(&p.id, "test_error", "it failed")
            .await
            .unwrap();

        let counts = inbox.counts().await.unwrap();
        assert_eq!(counts.processing, 0);
        assert_eq!(counts.failed, 1);
    }

    #[tokio::test]
    async fn requeue_moves_back_to_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();
        inbox.enqueue(&p).await.unwrap();
        inbox.claim_next().await.unwrap();

        inbox.requeue(&p.id).await.unwrap();

        let counts = inbox.counts().await.unwrap();
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.processing, 0);
    }

    // -------------------------------------------------------------------------
    // Pause / resume
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn pause_resume_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        assert!(!inbox.is_paused().await);
        inbox.set_paused(true).await.unwrap();
        assert!(inbox.is_paused().await);
        inbox.set_paused(false).await.unwrap();
        assert!(!inbox.is_paused().await);
    }

    // -------------------------------------------------------------------------
    // L12 — crash-recovery idempotence
    //
    // Simulate the crash window between finish_done writing the done/ payload
    // and the subsequent remove_file on the processing/ copy. Recovery must
    // detect the done+processing pair and discard the stale processing file
    // instead of re-queuing the already-completed item.
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn finish_cancelled_archives_to_done_not_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();
        inbox.enqueue(&p).await.unwrap();
        inbox.claim_next().await.unwrap();
        inbox.finish_cancelled(&p.id).await.unwrap();
        let counts = inbox.counts().await.unwrap();
        assert_eq!(counts.failed, 0, "a cancel must never hit the quarantine");
        assert_eq!(counts.processing, 0);
        assert_eq!(counts.done, 1);
    }

    #[tokio::test]
    async fn recover_orphans_skips_already_done_items() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();

        // Simulate a normal enqueue + claim.
        inbox.enqueue(&p).await.unwrap();
        inbox.claim_next().await.unwrap();

        // Simulate finish_done completing its atomic rename to done/ but
        // crashing before it removes the processing/ file.
        let done_path = tmp
            .path()
            .join("done")
            .join(format!("{}.json", p.id.as_str()));
        let json = serde_json::to_vec_pretty(&p).unwrap();
        tokio::fs::write(&done_path, &json).await.unwrap();
        // processing/ file is intentionally left in place.

        let counts_before = inbox.counts().await.unwrap();
        assert_eq!(
            counts_before.processing, 1,
            "setup: processing file must exist"
        );
        assert_eq!(counts_before.done, 1, "setup: done file must exist");

        // Recovery must drop the stale processing file, not re-queue the item.
        let recovered = inbox.recover_orphans().await.unwrap();
        assert!(
            recovered.is_empty(),
            "already-done item must not be recovered to pending"
        );

        let counts_after = inbox.counts().await.unwrap();
        assert_eq!(counts_after.pending, 0, "nothing should land in pending");
        assert_eq!(
            counts_after.processing, 0,
            "stale processing file must be removed"
        );
        assert_eq!(counts_after.done, 1, "done file must survive");
    }

    #[tokio::test]
    async fn recover_orphans_requeues_genuinely_interrupted_items() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();

        // A claim with no done/ means the pipeline was interrupted mid-run.
        inbox.enqueue(&p).await.unwrap();
        inbox.claim_next().await.unwrap();
        // No finish_done call — leave it in processing/.

        let recovered = inbox.recover_orphans().await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0], p.id);

        let counts = inbox.counts().await.unwrap();
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.processing, 0);
    }

    #[tokio::test]
    async fn cancel_pending_removes_item() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();
        inbox.enqueue(&p).await.unwrap();

        let removed = inbox.cancel_pending(&p.id).await.unwrap();
        assert!(removed);
        assert_eq!(inbox.counts().await.unwrap().pending, 0);

        // A second cancel on a gone item returns false (not an error).
        let removed2 = inbox.cancel_pending(&p.id).await.unwrap();
        assert!(!removed2);
    }

    #[tokio::test]
    async fn cancel_all_pending_clears_the_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        for _ in 0..3 {
            inbox.enqueue(&make_payload()).await.unwrap();
        }
        assert_eq!(inbox.counts().await.unwrap().pending, 3);

        let removed = inbox.cancel_all_pending().await.unwrap();
        assert_eq!(removed.len(), 3);
        assert_eq!(inbox.counts().await.unwrap().pending, 0);
    }

    #[tokio::test]
    async fn enqueue_uses_atomic_write_no_final_without_rename() {
        // The enqueue path must write via a .tmp file — the final .json must
        // not exist if the process crashes before the rename. We can't inject a
        // fault here, but we can assert that no stale .tmp remains after a
        // clean enqueue (the rename moved it).
        let tmp = tempfile::tempdir().unwrap();
        let inbox = open_inbox(tmp.path()).await;
        let p = make_payload();
        inbox.enqueue(&p).await.unwrap();

        let stale_tmp = tmp
            .path()
            .join("pending")
            .join(format!("{}.json.tmp", p.id.as_str()));
        assert!(
            !stale_tmp.exists(),
            "tmp file must not survive a clean enqueue"
        );
    }
}
