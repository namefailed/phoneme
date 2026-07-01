//! Library backup: zip the whole catalog out, restore it back in.
//!
//! A backup is a single zip with two parts:
//!
//! - **`catalog.json`** — a versioned envelope holding every recording's DTO
//!   (`phoneme_core::Recording`) plus the tag list, exactly the JSON the daemon's
//!   `ListRecordings` / `ListTags` produce.
//! - **`audio/<YYYY-MM-DD>/<HHmmssMMM>.wav`** — every `.wav` under the audio dir,
//!   each entry named relative to the audio dir so the day folder is preserved.
//!   Two recordings at the same ms-of-day on different days share a stem; naming
//!   the entry from the day folder keeps them from collapsing to one entry and
//!   clobbering each other on restore (a real data-loss case, regression-tested
//!   below).
//!
//! [`write_to_zip`] is the writer the export uses; [`restore_from_zip`] is its
//! inverse, driving `phoneme import-backup`. Restore is **idempotent**: a
//! recording whose id already exists in the target catalog is skipped (counted,
//! not overwritten), so re-importing the same backup never duplicates a row or
//! reverts a hand edit made since.
//!
//! Restore fidelity is bounded by what the backup captured. The DTO columns, the
//! tags, the per-recording entities, the auto-generated chapters, and the
//! whole-meeting digests round-trip; machine-truth side tables the export never
//! wrote (`original_transcript` / `clean_transcript`, transcript segments +
//! words, embeddings, voiceprints, AI-activity, custom speaker names) do not, so
//! the restored recording is whatever the backup said it was, and a
//! re-transcribe regenerates the derived data.
//!
//! Whole-meeting digests are a side table keyed by `meeting_id` (not a
//! [`Recording`] DTO column, since a meeting isn't one row), so they ride
//! alongside the recordings in their own manifest array rather than on any track:
//! the export captures them (over IPC, mirroring the tag list) and restore
//! replays each via the idempotent [`Catalog::update_meeting_digest`] upsert.

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::id::RecordingId;
use crate::tags::Tag;
use crate::types::{Chapter, MeetingDigest, PeriodDigest, Recording};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The backup envelope's schema version. Bumped only on a breaking layout
/// change; [`restore_from_zip`] refuses a newer one rather than mis-reading it.
pub const BACKUP_VERSION: u32 = 1;

/// The catalog metadata entry name inside the zip.
const CATALOG_ENTRY: &str = "catalog.json";

/// The prefix under which audio files live inside the zip.
const AUDIO_PREFIX: &str = "audio/";

/// Hard cap on the decompressed size of a single audio entry we'll read on
/// restore. The entry's self-reported `size()` is attacker-controllable (a zip
/// bomb claims a tiny size while expanding to GiB), so the read itself is bounded
/// to this — not the advertised header — and a stream that runs past it is
/// rejected (a DoS guard, not a real-recording limit — 2 GiB is far past any
/// plausible WAV).
const MAX_RESTORE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One recording's auto-generated chapters, carried in the manifest's own array.
///
/// Chapters are a per-recording child table (keyed by `recording_id`), not a
/// [`Recording`] DTO column, so they ride alongside the recordings in their own
/// manifest array rather than on the track — mirroring how the meeting digests
/// are carried. Replayed via [`Catalog::replace_chapters`] on restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingChapters {
    /// The recording these chapters belong to.
    pub recording_id: RecordingId,
    /// The recording's chapters in chronological order.
    pub chapters: Vec<Chapter>,
}

/// Which of a recording's tasks/entities were user-added (`source = 'manual'`)
/// at export time, keyed the way the setters dedupe (task `text`, entity
/// `(kind, value)`). Carried out-of-band like chapters because the
/// `Task`/`Entity` DTOs don't serialize `source`: the restore setters insert
/// everything as `'llm'`, and without flipping these keys back to manual the
/// first re-extraction's `DELETE ... WHERE source='llm'` would silently remove
/// user-owned rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualSources {
    /// The recording the manual rows belong to.
    pub recording_id: RecordingId,
    /// The `text` of each `source='manual'` task.
    #[serde(default)]
    pub task_texts: Vec<String>,
    /// The `(kind, value)` of each `source='manual'` entity.
    #[serde(default)]
    pub entity_keys: Vec<(String, String)>,
}

/// The deserialized `catalog.json` envelope.
///
/// `recordings`/`tags`/`meeting_digests` are the same DTOs the IPC layer emits,
/// so the envelope is just those arrays under a version tag. `#[serde(default)]`
/// on `tags` and `meeting_digests` keeps an older or hand-written backup that
/// omits them readable (a pre-digest backup simply restores no digests).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Envelope schema version — must be `<= BACKUP_VERSION` to restore.
    pub version: u32,
    /// Every recording DTO captured in the backup.
    pub recordings: Vec<Recording>,
    /// The tag list (id/name/color). Re-created by name on restore.
    #[serde(default)]
    pub tags: Vec<Tag>,
    /// The whole-meeting digests, one per meeting. A side table keyed by
    /// `meeting_id` (not a [`Recording`] column), so it's carried here and
    /// replayed via [`Catalog::update_meeting_digest`] on restore.
    #[serde(default)]
    pub meeting_digests: Vec<MeetingDigest>,
    /// The period digests, one per date range. A side table keyed by range
    /// (not a [`Recording`] column), so it's carried here and replayed via
    /// [`Catalog::update_period_digest`] on restore. `#[serde(default)]` keeps a
    /// pre-period-digest backup readable (it simply restores none).
    #[serde(default)]
    pub period_digests: Vec<PeriodDigest>,
    /// The auto-generated chapters, one entry per recording that has any. A
    /// per-recording side table (not a [`Recording`] column), so it's carried
    /// here and replayed via [`Catalog::replace_chapters`] on restore.
    /// `#[serde(default)]` keeps a pre-chapters backup readable (restores none).
    #[serde(default)]
    pub chapters: Vec<RecordingChapters>,
    /// Which task/entity rows were user-added (`source='manual'`) at export
    /// time, so restore can flip them back after the setters insert everything
    /// as `'llm'`. `#[serde(default)]` keeps older backups readable (nothing to
    /// flip — matching their pre-fix behavior).
    #[serde(default)]
    pub manual_sources: Vec<ManualSources>,
}

/// Serialize-only twin of [`BackupManifest`] that borrows the caller's slices,
/// so the writer never clones the whole library just to serialize it. Field
/// names and order match the owned struct exactly — the JSON is identical.
#[derive(Serialize)]
struct BackupManifestRef<'a> {
    version: u32,
    recordings: &'a [Recording],
    tags: &'a [Tag],
    meeting_digests: &'a [MeetingDigest],
    period_digests: &'a [PeriodDigest],
    chapters: &'a [RecordingChapters],
    manual_sources: &'a [ManualSources],
}

/// What a [`restore_from_zip`] did: how many recordings were newly imported and
/// how many were skipped because their id already existed in the target catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RestoreReport {
    /// Recordings inserted into the catalog (and whose audio was copied in).
    pub imported: usize,
    /// Recordings skipped because their id was already present (idempotency).
    pub skipped: usize,
}

/// Zip-entry name for one audio file, preserving its day folder.
///
/// Recordings live at `<audio_dir>/<YYYY-MM-DD>/<HHmmssMMM>.wav` and the stem is
/// time-of-day only, so two recordings at the same ms-of-day on different days
/// share a stem. Naming the entry from the path relative to `audio_dir` (with
/// backslashes normalized to `/` for a portable archive) keeps the day folder,
/// so the two never collide to one entry. Falls back to the bare filename if the
/// path isn't under `audio_dir`.
fn audio_entry_name(audio_dir: &Path, path: &Path) -> String {
    match path.strip_prefix(audio_dir) {
        Ok(rel) => format!("{AUDIO_PREFIX}{}", rel.to_string_lossy().replace('\\', "/")),
        Err(_) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{AUDIO_PREFIX}{name}")
        }
    }
}

/// Write a backup zip: the `catalog.json` envelope plus every `.wav` under
/// `audio_dir`, into a freshly created file at `out`.
///
/// The recordings/tags/meeting digests/period digests/chapters are supplied by
/// the caller (the daemon-driven export fetches them over IPC; the round-trip
/// test reads them straight from a `Catalog`), so this owns only the archive
/// format — the one place the layout is defined for both directions.
pub fn write_to_zip(
    recordings: &[Recording],
    tags: &[Tag],
    meeting_digests: &[MeetingDigest],
    period_digests: &[PeriodDigest],
    chapters: &[RecordingChapters],
    manual_sources: &[ManualSources],
    audio_dir: &Path,
    out: &Path,
) -> Result<()> {
    // Borrowing manifest: serializes the caller's slices directly instead of
    // cloning the whole library into an owned envelope first.
    let manifest = BackupManifestRef {
        version: BACKUP_VERSION,
        recordings,
        tags,
        meeting_digests,
        period_digests,
        chapters,
        manual_sources,
    };
    let json_bytes = serde_json::to_vec_pretty(&manifest)?;

    let file = std::fs::File::create(out)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file(CATALOG_ENTRY, options)
        .map_err(|e| Error::Internal(format!("backup: writing {CATALOG_ENTRY}: {e}")))?;
    zip.write_all(&json_bytes)?;

    // Walk the audio dir depth-first, packing every .wav with a day-folder-
    // preserving entry name. A read error on one file warns and skips rather
    // than aborting the whole backup.
    if audio_dir.exists() {
        let mut stack = vec![audio_dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("backup: skipping unreadable dir {}: {e}", dir.display());
                    continue;
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let is_wav = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
                if !is_wav {
                    continue;
                }
                let entry_name = audio_entry_name(audio_dir, &path);
                // Open first so an unreadable file warns and skips before we've
                // started its zip entry.
                let mut file = match std::fs::File::open(&path) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!("backup: skipping unreadable {}: {e}", path.display());
                        continue;
                    }
                };
                zip.start_file(&entry_name, options)
                    .map_err(|e| Error::Internal(format!("backup: writing {entry_name}: {e}")))?;
                // Stream the WAV straight into the archive rather than buffering the
                // whole file in memory.
                std::io::copy(&mut file, &mut zip)?;
            }
        }
    }

    zip.finish()
        .map_err(|e| Error::Internal(format!("backup: finalizing zip: {e}")))?;
    Ok(())
}

/// Restore a backup zip into `catalog` + `audio_dir`, returning the
/// imported/skipped counts.
///
/// For each recording in `catalog.json`: if its id already exists in the target
/// catalog it is skipped (idempotent — a re-import never duplicates or reverts).
/// Otherwise its row is inserted with every persisted column ([`Catalog::
/// insert_restored`]), its audio entry (named from the id's day folder + stem)
/// is copied into `audio_dir`, and its tags are re-created by name and attached.
/// The stored `audio_path` is rewritten to point at the restored file under the
/// target audio dir, since the backup may be restored on a different machine.
pub async fn restore_from_zip(
    zip_path: &Path,
    catalog: &Catalog,
    audio_dir: &Path,
) -> Result<RestoreReport> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| Error::Internal(format!("backup: opening {}: {e}", zip_path.display())))?;

    // Read the manifest first.
    let manifest: BackupManifest = {
        let mut entry = archive.by_name(CATALOG_ENTRY).map_err(|e| {
            Error::Internal(format!(
                "backup: {} has no {CATALOG_ENTRY} ({e}) — not a phoneme backup?",
                zip_path.display()
            ))
        })?;
        let mut json = String::new();
        entry.read_to_string(&mut json)?;
        serde_json::from_str(&json)?
    };
    if manifest.version > BACKUP_VERSION {
        return Err(Error::Internal(format!(
            "backup: archive version {} is newer than this build supports ({BACKUP_VERSION}) — upgrade phoneme",
            manifest.version
        )));
    }

    std::fs::create_dir_all(audio_dir)?;
    let mut report = RestoreReport::default();

    for rec in &manifest.recordings {
        // Skip ids that already exist — idempotent re-import, never a clobber.
        if catalog.get(&rec.id).await?.is_some() {
            report.skipped += 1;
            continue;
        }

        // Copy this recording's audio (if the backup carried it) to the target
        // audio dir, and capture where it landed so the row points at it.
        //
        // Ordering is deliberate: audio first, then the row. Row + file can't share
        // one transaction (SQLite vs filesystem), so a crash between them must fail
        // safe. This order leaves at worst an orphan WAV with no row referencing it
        // — invisible, and self-healing: a re-run finds the id absent (the insert
        // never happened), re-extracts over the file, and inserts. The inverse
        // (row first) would leave a visible row pointing at missing audio that a
        // re-run skips (id now present) and never heals, which is strictly worse.
        let restored_audio_path = restore_audio_for(&mut archive, &rec.id, audio_dir)?;

        // Insert the row with the audio path rewritten to the restored location
        // (empty when no audio was in the backup — retention may have reclaimed
        // it, leaving the row). Clone so we don't mutate the manifest.
        let mut row = rec.clone();
        if let Some(p) = restored_audio_path {
            row.audio_path = p.to_string_lossy().into_owned();
        }
        catalog.insert_restored(&row).await?;

        // Re-create tags by name (idempotent get-or-create) and attach them.
        for tag in &rec.tags {
            let created = catalog.add_tag(&tag.name, tag.color.as_deref()).await?;
            catalog.attach_tag(&rec.id, created.id).await?;
        }

        // Restore the recording's structured entities (the `entities` child table
        // the DTO carries). `set_entities` replaces wholesale, so the freshly
        // inserted row's entities land exactly as exported.
        if !rec.entities.is_empty() {
            catalog.set_entities(&rec.id, &rec.entities).await?;
        }

        // Restore the recording's tasks (the `tasks` child table the DTO carries),
        // INCLUDING each task's completed flag — `set_tasks` carries `done` from
        // the DTO (a freshly inserted row has no prior tasks to merge against). The
        // export already serializes `tasks` on every Recording (list/get populate
        // it); without this the restore silently dropped them, unlike entities.
        if !rec.tasks.is_empty() {
            catalog.set_tasks(&rec.id, &rec.tasks).await?;
        }

        // Flip the user-added rows back to `source='manual'`. The setters above
        // insert everything as 'llm'; without this the first re-extraction's
        // `DELETE ... WHERE source='llm'` would remove the user's own rows.
        // Older backups carry no manual_sources — nothing to flip.
        if let Some(ms) = manifest
            .manual_sources
            .iter()
            .find(|m| m.recording_id == rec.id)
        {
            if !ms.task_texts.is_empty() {
                catalog.mark_tasks_manual(&rec.id, &ms.task_texts).await?;
            }
            if !ms.entity_keys.is_empty() {
                catalog
                    .mark_entities_manual(&rec.id, &ms.entity_keys)
                    .await?;
            }
        }

        // Restore the recording's auto-generated chapters (a per-recording side
        // table keyed by id, carried in the manifest's own array rather than on
        // the DTO). `replace_chapters` writes them wholesale onto the freshly
        // inserted row; skip empties like the entities/tasks restore above.
        if let Some(rc) = manifest
            .chapters
            .iter()
            .find(|c| c.recording_id == rec.id && !c.chapters.is_empty())
        {
            catalog.replace_chapters(&rec.id, &rc.chapters).await?;
        }

        report.imported += 1;
    }

    // Replay the whole-meeting digests (the side table keyed by `meeting_id`,
    // carried in the manifest rather than on any track). Idempotent like the
    // recordings: a digest is only written when the target has none for that
    // meeting, so a re-import never clobbers a digest regenerated since restore —
    // matching the "skip existing ids / never revert a hand edit" guarantee.
    for digest in &manifest.meeting_digests {
        if catalog.meeting_digest(&digest.meeting_id).await?.is_some() {
            continue;
        }
        catalog
            .update_meeting_digest(
                &digest.meeting_id,
                &digest.digest,
                digest.digest_model.as_deref(),
            )
            .await?;
    }

    // Replay the period digests (the side table keyed by range, carried in the
    // manifest rather than on any recording). Idempotent like the meeting
    // digests: a digest is only written when the target has none for that range,
    // so a re-import never clobbers a digest regenerated since restore.
    for digest in &manifest.period_digests {
        if catalog.period_digest(&digest.key).await?.is_some() {
            continue;
        }
        catalog
            .update_period_digest(
                &digest.key,
                &digest.label,
                digest.since,
                digest.until,
                &digest.digest,
                digest.digest_model.as_deref(),
                digest.source_count,
            )
            .await?;
    }

    Ok(report)
}

/// Extract one recording's `.wav` from the archive into `audio_dir`, under the
/// id's day folder, returning the path it was written to. `Ok(None)` when the
/// backup contains no audio entry for this id (a row whose audio retention
/// already reclaimed). The id's fixed day-folder/stem layout is what locates the
/// entry, so a row's stored `audio_path` (which may be an absolute path from
/// another machine) is never trusted here.
fn restore_audio_for(
    archive: &mut zip::ZipArchive<std::fs::File>,
    id: &RecordingId,
    audio_dir: &Path,
) -> Result<Option<PathBuf>> {
    let entry_name = format!("{AUDIO_PREFIX}{}/{}.wav", id.day_folder(), id.file_stem());

    // `by_name` borrows the archive; read the bytes out before touching the
    // filesystem so the borrow ends cleanly.
    let bytes = match archive.by_name(&entry_name) {
        Ok(entry) => {
            // The entry's self-reported size is untrusted (a hand-crafted ZIP can
            // claim a tiny one while the deflate stream expands to GiB — a zip
            // bomb). Use size() only as a fast-path early-out, then bound the
            // actual read: take MAX_RESTORE_BYTES + 1 and reject if we got past
            // the cap, so the decompressed bytes — not the advertised header — are
            // what the limit guards.
            if entry.size() > MAX_RESTORE_BYTES {
                return Err(Error::Internal(format!(
                    "backup: entry {entry_name} too large ({} bytes > {MAX_RESTORE_BYTES} cap)",
                    entry.size()
                )));
            }
            let mut buf = Vec::new();
            entry.take(MAX_RESTORE_BYTES + 1).read_to_end(&mut buf)?;
            if buf.len() as u64 > MAX_RESTORE_BYTES {
                return Err(Error::Internal(format!(
                    "backup: entry {entry_name} decompressed past the {MAX_RESTORE_BYTES} byte cap"
                )));
            }
            buf
        }
        // No audio for this id — a row whose audio was reclaimed. Not an error.
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => {
            return Err(Error::Internal(format!(
                "backup: reading {entry_name}: {e}"
            )))
        }
    };

    let day_dir = audio_dir.join(id.day_folder());
    std::fs::create_dir_all(&day_dir)?;
    let dest = day_dir.join(format!("{}.wav", id.file_stem()));
    std::fs::write(&dest, &bytes)?;
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RecordingStatus;
    use chrono::{Local, TimeZone};
    use std::collections::HashMap;

    /// A recording fixed to a known datetime so its id's day folder + stem are
    /// deterministic (the audio entry name is derived from them).
    fn rec_at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Recording {
        let dt = Local.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap();
        let id = RecordingId::from_datetime(dt);
        Recording {
            id: id.clone(),
            started_at: dt,
            duration_ms: 4200,
            audio_path: format!("/some/where/{}/{}.wav", id.day_folder(), id.file_stem()),
            transcript: Some("hello from the backup".into()),
            model: Some("ggml-base.en".into()),
            status: RecordingStatus::Done,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: Some(dt),
            hook_ran_at: None,
            notes: Some("a note".into()),
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: Some("llama3.2:3b".into()),
            diarized: false,
            user_edited: true,
            favorite: true,
            pinned: true,
            tag_suggestions: vec![],
            summary: Some("short summary".into()),
            summary_model: Some("phi3:mini".into()),
            entities_model: None,
            chapters_model: None,
            tasks_model: None,
            title: Some("My Title".into()),
            title_is_auto: false,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            mean_confidence: Some(0.82),
            detected_language: None,
            ext_ref: None,
            tags: vec![],
            entities: vec![],
            tasks: vec![],
            speaker_names: vec![],
        }
    }

    /// Write a tiny valid-ish WAV-named file at the id's on-disk location.
    fn write_audio(audio_dir: &Path, rec: &Recording, contents: &[u8]) -> PathBuf {
        let day = audio_dir.join(rec.id.day_folder());
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join(format!("{}.wav", rec.id.file_stem()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn audio_entry_name_preserves_the_day_folder() {
        let audio_dir = Path::new("/data/audio");
        let path = Path::new("/data/audio/2026-05-19/143500042.wav");
        assert_eq!(
            audio_entry_name(audio_dir, path),
            "audio/2026-05-19/143500042.wav"
        );
    }

    #[test]
    fn audio_entry_name_normalizes_windows_separators() {
        let audio_dir = Path::new(r"C:\data\audio");
        let path = Path::new(r"C:\data\audio\2026-05-19\143500042.wav");
        let name = audio_entry_name(audio_dir, path);
        assert!(!name.contains('\\'), "no backslashes in entry name: {name}");
        assert_eq!(name, "audio/2026-05-19/143500042.wav");
    }

    #[tokio::test]
    async fn round_trip_export_then_import_restores_recordings() {
        let tmp = tempfile::tempdir().unwrap();
        let src_audio = tmp.path().join("src-audio");

        // Seed a source catalog with two recordings (one tagged) + audio files.
        let src = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r1 = rec_at(2026, 5, 19, 14, 35, 0);
        let r2 = rec_at(2026, 5, 20, 9, 0, 0);
        src.insert_restored(&r1).await.unwrap();
        src.insert_restored(&r2).await.unwrap();
        let work = src.add_tag("Work", Some("#4caf50")).await.unwrap();
        src.attach_tag(&r1.id, work.id).await.unwrap();
        // Seed structured tasks on r1, one COMPLETED, so the round-trip proves the
        // tasks child table — and the user's `done` flag — survive a backup (the
        // restore path that entities had but tasks was missing).
        src.set_tasks(
            &r1.id,
            &[
                crate::Task {
                    id: 0,
                    text: "Ship the release".into(),
                    due_hint: Some("Friday".into()),
                    done: true,
                },
                crate::Task {
                    id: 0,
                    text: "Write the changelog".into(),
                    due_hint: None,
                    done: false,
                },
            ],
        )
        .await
        .unwrap();
        // Seed auto-generated chapters on r1 (a per-recording side table keyed by
        // id, not a DTO column) so the round-trip proves they survive a backup —
        // the data-loss case this whole change closes.
        src.replace_chapters(
            &r1.id,
            &[
                crate::Chapter {
                    start_ms: 0,
                    end_ms: 60_000,
                    title: "Intro".into(),
                    summary: Some("kickoff".into()),
                },
                crate::Chapter {
                    start_ms: 60_000,
                    end_ms: 120_000,
                    title: "Deep dive".into(),
                    summary: None,
                },
            ],
        )
        .await
        .unwrap();
        let a1 = write_audio(&src_audio, &r1, b"RIFF-one-audio");
        let a2 = write_audio(&src_audio, &r2, b"RIFF-two-audio");
        assert!(a1.exists() && a2.exists());

        // Seed a whole-meeting digest (a side table keyed by meeting_id, not a
        // Recording column) so the round-trip below proves it survives.
        src.update_meeting_digest("meeting-xyz", "Overview: shipped v2.", Some("llama3.2:3b"))
            .await
            .unwrap();

        // Seed a period digest (its own side table keyed by range) so the
        // round-trip proves it survives too.
        let p_since = Local.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap();
        let p_until = Local.with_ymd_and_hms(2026, 5, 20, 23, 59, 59).unwrap();
        src.update_period_digest(
            "period-key-1",
            "week of 2026-05-19",
            p_since,
            p_until,
            "Rollup: two recordings; one decision; one open item.",
            Some("phi3:mini"),
            2,
        )
        .await
        .unwrap();

        // Export to a temp zip via the shared writer (the same archive format
        // the CLI export emits).
        let zip_path = tmp.path().join("backup.zip");
        let recordings = src.list(&Default::default()).await.unwrap();
        let tags = src.list_all_tags().await.unwrap();
        let digests = src.list_all_meeting_digests().await.unwrap();
        let period_digests = src.list_all_period_digests().await.unwrap();
        // Gather each recording's chapters the way the export does (per-recording
        // read), keeping only those that have any.
        let mut chapters = Vec::new();
        for rec in &recordings {
            let cs = src.chapters_for(&rec.id).await.unwrap();
            if !cs.is_empty() {
                chapters.push(RecordingChapters {
                    recording_id: rec.id.clone(),
                    chapters: cs,
                });
            }
        }
        write_to_zip(
            &recordings,
            &tags,
            &digests,
            &period_digests,
            &chapters,
            &[],
            &src_audio,
            &zip_path,
        )
        .unwrap();
        assert!(zip_path.exists());

        // Import into a FRESH catalog + audio dir.
        let dst_audio = tmp.path().join("dst-audio");
        let dst = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let report = restore_from_zip(&zip_path, &dst, &dst_audio).await.unwrap();
        assert_eq!(report.imported, 2);
        assert_eq!(report.skipped, 0);

        // Both recordings are back, by id, with their persisted fields intact.
        let restored = dst.list(&Default::default()).await.unwrap();
        assert_eq!(restored.len(), 2);
        let by_id: HashMap<_, _> = restored.iter().map(|r| (r.id.as_str(), r)).collect();

        let g1 = by_id.get(r1.id.as_str()).expect("r1 restored");
        assert_eq!(g1.transcript.as_deref(), Some("hello from the backup"));
        assert_eq!(g1.title.as_deref(), Some("My Title"));
        assert!(!g1.title_is_auto);
        assert!(g1.favorite);
        assert!(g1.pinned);
        assert!(g1.user_edited);
        assert_eq!(g1.summary.as_deref(), Some("short summary"));
        assert_eq!(g1.summary_model.as_deref(), Some("phi3:mini"));
        assert_eq!(g1.cleanup_model.as_deref(), Some("llama3.2:3b"));
        assert_eq!(g1.notes.as_deref(), Some("a note"));
        // The audio path was rewritten to the restored file under the target dir.
        let restored_a1 = dst_audio
            .join(r1.id.day_folder())
            .join(format!("{}.wav", r1.id.file_stem()));
        assert_eq!(g1.audio_path, restored_a1.to_string_lossy());
        assert_eq!(std::fs::read(&restored_a1).unwrap(), b"RIFF-one-audio");
        // The tag came back too.
        assert_eq!(g1.tags.len(), 1);
        assert_eq!(g1.tags[0].name, "Work");
        assert_eq!(g1.tags[0].color.as_deref(), Some("#4caf50"));

        // The tasks came back — including the completed flag and due hint.
        assert_eq!(g1.tasks.len(), 2, "both tasks must survive the round-trip");
        let ship = g1
            .tasks
            .iter()
            .find(|t| t.text == "Ship the release")
            .expect("completed task restored");
        assert!(ship.done, "a completed task must survive export/import");
        assert_eq!(ship.due_hint.as_deref(), Some("Friday"));
        assert!(
            g1.tasks
                .iter()
                .any(|t| t.text == "Write the changelog" && !t.done),
            "an open task stays open"
        );

        // The auto-generated chapters round-tripped: they rode in the manifest's
        // own array (no Recording column carries them) and were replayed via
        // replace_chapters on restore. Read them back through the catalog since
        // the DTO doesn't surface them.
        let restored_chapters = dst.chapters_for(&r1.id).await.unwrap();
        assert_eq!(
            restored_chapters.len(),
            2,
            "both chapters must survive the round-trip"
        );
        assert_eq!(restored_chapters[0].title, "Intro");
        assert_eq!(restored_chapters[0].start_ms, 0);
        assert_eq!(restored_chapters[0].end_ms, 60_000);
        assert_eq!(restored_chapters[0].summary.as_deref(), Some("kickoff"));
        assert_eq!(restored_chapters[1].title, "Deep dive");
        assert_eq!(restored_chapters[1].start_ms, 60_000);
        assert_eq!(restored_chapters[1].summary, None);

        // r2's audio also landed (different day folder, no collision).
        let restored_a2 = dst_audio
            .join(r2.id.day_folder())
            .join(format!("{}.wav", r2.id.file_stem()));
        assert_eq!(std::fs::read(&restored_a2).unwrap(), b"RIFF-two-audio");

        // The whole-meeting digest round-tripped: it rode in the manifest's own
        // array (no Recording column carries it) and was replayed on restore.
        let restored_digest = dst.meeting_digest("meeting-xyz").await.unwrap().unwrap();
        assert_eq!(restored_digest.digest, "Overview: shipped v2.");
        assert_eq!(restored_digest.digest_model.as_deref(), Some("llama3.2:3b"));

        // The period digest round-tripped too (its own manifest array, replayed
        // by key on restore), with every field intact.
        let restored_period = dst.period_digest("period-key-1").await.unwrap().unwrap();
        assert_eq!(restored_period.label, "week of 2026-05-19");
        assert_eq!(
            restored_period.digest,
            "Rollup: two recordings; one decision; one open item."
        );
        assert_eq!(restored_period.digest_model.as_deref(), Some("phi3:mini"));
        assert_eq!(restored_period.source_count, 2);
        assert_eq!(restored_period.since, p_since);
        assert_eq!(restored_period.until, p_until);
    }

    /// A user-added ('manual') task and entity must survive backup → restore →
    /// the NEXT re-extraction. The restore setters insert everything as 'llm';
    /// the manifest's `manual_sources` array is what flips the user rows back,
    /// so a later `set_tasks`/`set_entities` (whose DELETE only targets 'llm')
    /// can't remove them. This is the round-trip half of the manual/llm split.
    #[tokio::test]
    async fn manual_tasks_and_entities_survive_restore_then_reextraction() {
        let tmp = tempfile::tempdir().unwrap();
        let src_audio = tmp.path().join("src-audio");
        let src = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r1 = rec_at(2026, 5, 19, 14, 35, 0);
        src.insert_restored(&r1).await.unwrap();
        write_audio(&src_audio, &r1, b"RIFF-manual-audio");

        // One extracted set + one user-added row each.
        src.set_tasks(
            &r1.id,
            &[crate::Task {
                id: 0,
                text: "Ship the release".into(),
                due_hint: None,
                done: false,
            }],
        )
        .await
        .unwrap();
        src.add_task(&r1.id, "Call Alice", Some("tomorrow"))
            .await
            .unwrap();
        src.set_entities(
            &r1.id,
            &[crate::Entity {
                kind: "person".into(),
                value: "Bob".into(),
            }],
        )
        .await
        .unwrap();
        src.add_entity(&r1.id, "person", "Alice").await.unwrap();

        // Export the way the CLI does: recordings + the manual-source keys.
        let recordings = src.list(&Default::default()).await.unwrap();
        let manual_tasks = src.manual_task_texts_all().await.unwrap();
        let mut manual_entities = src.manual_entity_keys_all().await.unwrap();
        let manual_sources: Vec<ManualSources> = manual_tasks
            .into_iter()
            .map(|(rid, task_texts)| {
                let entity_keys = manual_entities.remove(&rid).unwrap_or_default();
                ManualSources {
                    recording_id: rid,
                    task_texts,
                    entity_keys,
                }
            })
            .collect();
        assert_eq!(manual_sources.len(), 1, "one recording carries manual rows");
        assert_eq!(manual_sources[0].task_texts, vec!["Call Alice".to_string()]);
        assert_eq!(
            manual_sources[0].entity_keys,
            vec![("person".to_string(), "Alice".to_string())]
        );

        let zip_path = tmp.path().join("backup.zip");
        write_to_zip(
            &recordings,
            &[],
            &[],
            &[],
            &[],
            &manual_sources,
            &src_audio,
            &zip_path,
        )
        .unwrap();

        // Restore into a FRESH catalog, then run a re-extraction with a
        // DIFFERENT llm set — the deletion the manual rows must survive.
        let dst_audio = tmp.path().join("dst-audio");
        let dst = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        restore_from_zip(&zip_path, &dst, &dst_audio).await.unwrap();
        dst.set_tasks(
            &r1.id,
            &[crate::Task {
                id: 0,
                text: "Totally new extracted task".into(),
                due_hint: None,
                done: false,
            }],
        )
        .await
        .unwrap();
        dst.set_entities(
            &r1.id,
            &[crate::Entity {
                kind: "topic".into(),
                value: "release planning".into(),
            }],
        )
        .await
        .unwrap();

        let tasks = dst.list_tasks(&r1.id).await.unwrap();
        let task_texts: Vec<&str> = tasks.iter().map(|t| t.text.as_str()).collect();
        assert!(
            task_texts.contains(&"Call Alice"),
            "the manual task must survive re-extraction, got {task_texts:?}"
        );
        assert!(
            !task_texts.contains(&"Ship the release"),
            "the old llm task is replaced by the new extraction"
        );
        let entities = dst.list_entities(&r1.id).await.unwrap();
        let entity_keys: Vec<(&str, &str)> = entities
            .iter()
            .map(|e| (e.kind.as_str(), e.value.as_str()))
            .collect();
        assert!(
            entity_keys.contains(&("person", "Alice")),
            "the manual entity must survive re-extraction, got {entity_keys:?}"
        );
        assert!(
            !entity_keys.contains(&("person", "Bob")),
            "the old llm entity is replaced by the new extraction"
        );
    }

    #[tokio::test]
    async fn reimport_is_idempotent_and_skips_existing_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let src_audio = tmp.path().join("audio");
        let src = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r1 = rec_at(2026, 5, 19, 14, 35, 0);
        src.insert_restored(&r1).await.unwrap();
        write_audio(&src_audio, &r1, b"audio-bytes");

        let zip_path = tmp.path().join("backup.zip");
        let recordings = src.list(&Default::default()).await.unwrap();
        write_to_zip(&recordings, &[], &[], &[], &[], &[], &src_audio, &zip_path).unwrap();

        let dst_audio = tmp.path().join("dst");
        let dst = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

        let first = restore_from_zip(&zip_path, &dst, &dst_audio).await.unwrap();
        assert_eq!(first.imported, 1);
        assert_eq!(first.skipped, 0);

        // A hand edit after the first import must survive a second import.
        dst.update_user_transcript(&r1.id, "edited since restore")
            .await
            .unwrap();

        let second = restore_from_zip(&zip_path, &dst, &dst_audio).await.unwrap();
        assert_eq!(second.imported, 0, "the id already exists");
        assert_eq!(second.skipped, 1);

        // Still exactly one row, and the post-restore edit survived intact.
        let rows = dst.list(&Default::default()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].transcript.as_deref(), Some("edited since restore"));
    }

    #[tokio::test]
    async fn same_ms_different_day_recordings_dont_collide() {
        // The data-loss case: two ids that share a ms-of-day stem but differ by
        // day must each restore their own audio.
        let tmp = tempfile::tempdir().unwrap();
        let src_audio = tmp.path().join("audio");
        let src = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

        // Force identical stems on different days by reusing the same datetime
        // shape — `from_datetime`'s monotonic suffix differs, so to truly collide
        // stems we build ids that share the time-of-day portion. Relying on two
        // real recordings whose `file_stem` happens to match would be fragile;
        // instead assert the entry names differ, which is what guards the archive.
        let r_day1 = rec_at(2026, 5, 19, 14, 35, 0);
        let r_day2 = rec_at(2026, 5, 20, 14, 35, 0);
        src.insert_restored(&r_day1).await.unwrap();
        src.insert_restored(&r_day2).await.unwrap();
        write_audio(&src_audio, &r_day1, b"day-one");
        write_audio(&src_audio, &r_day2, b"day-two");

        let zip_path = tmp.path().join("backup.zip");
        let recordings = src.list(&Default::default()).await.unwrap();
        write_to_zip(&recordings, &[], &[], &[], &[], &[], &src_audio, &zip_path).unwrap();

        let dst_audio = tmp.path().join("dst");
        let dst = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        restore_from_zip(&zip_path, &dst, &dst_audio).await.unwrap();

        let a1 = dst_audio
            .join(r_day1.id.day_folder())
            .join(format!("{}.wav", r_day1.id.file_stem()));
        let a2 = dst_audio
            .join(r_day2.id.day_folder())
            .join(format!("{}.wav", r_day2.id.file_stem()));
        assert_eq!(std::fs::read(&a1).unwrap(), b"day-one");
        assert_eq!(std::fs::read(&a2).unwrap(), b"day-two");
    }

    #[tokio::test]
    async fn rejects_a_newer_archive_version() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("future.zip");
        // Hand-write an envelope claiming a version from the future.
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file(CATALOG_ENTRY, opts).unwrap();
        zip.write_all(br#"{"version": 999, "recordings": [], "tags": []}"#)
            .unwrap();
        zip.finish().unwrap();

        let dst = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let dst_audio = tmp.path().join("dst");
        let err = restore_from_zip(&zip_path, &dst, &dst_audio)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("newer than this build"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn errors_on_a_zip_without_a_catalog_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("notabackup.zip");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("random.txt", opts).unwrap();
        zip.write_all(b"nope").unwrap();
        zip.finish().unwrap();

        let dst = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let dst_audio = tmp.path().join("dst");
        let err = restore_from_zip(&zip_path, &dst, &dst_audio)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains(CATALOG_ENTRY),
            "unexpected error: {err}"
        );
    }
}
