use crate::error::{Error, Result};
use crate::id::RecordingId;
use crate::types::HookPayload;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Which directory of the inbox a payload lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxState {
    Pending,
    Processing,
    Done,
    Failed,
}

impl InboxState {
    pub fn subdir(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Count of payloads in each inbox state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InboxCounts {
    pub pending: usize,
    pub processing: usize,
    pub done: usize,
    pub failed: usize,
}

/// Failure payload (written when finish_failed is called).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedPayload {
    pub id: RecordingId,
    pub error_kind: String,
    pub error_message: String,
}

/// Filesystem-backed work queue.
///
/// Each payload lives as a single JSON file under one of four subdirectories
/// (`pending/`, `processing/`, `done/`, `failed/`). State transitions are
/// atomic file renames, which on the same filesystem are crash-safe — either
/// the rename happened or it didn't.
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

    /// Claim the oldest pending payload (moving it to `processing/`).
    /// Returns `None` if there's nothing pending.
    ///
    /// A corrupt (unparseable) payload is claimed exactly once, quarantined to
    /// `failed/`, and reported as `Ok(None)` so the caller simply tries the
    /// next file. The rename-before-parse ordering is what makes this work: if
    /// we parsed first, a single corrupt file at the head of the queue would
    /// fail every `claim_next()` call forever and starve every file behind it.
    pub async fn claim_next(&self) -> Result<Option<HookPayload>> {
        let pending = self.root.join("pending");
        let entries = read_json_entries_sorted(&pending).await?;
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
        for path in read_json_entries_sorted(&self.root.join("pending")).await? {
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
