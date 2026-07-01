//! `phoneme export` — two export modes behind one command.
//!
//! **Library zip** (`phoneme export <FILE>`): fetches every recording
//! (`ListRecordings` with the default filter), the tag list (`ListTags`),
//! the whole-meeting digests (`ListMeetingDigests`), and each recording's
//! auto-generated chapters (`GetChapters`), writes them as `catalog.json`
//! (versioned envelope), and packs every `.wav` under the configured audio
//! dir into `audio/` — a portable backup of the whole library.
//!
//! **Captions** (`phoneme export --captions <ID> [--format srt|vtt]
//! [--out FILE|-]`): fetches the recording's machine segments
//! (`GetSegments`) and renders them through `phoneme_core::export` into
//! SRT (default) or WebVTT, written to `--out` (`-` = stdout) or
//! `<id>.<ext>` in the current directory. Exits 7 with a "retranscribe to
//! generate them" hint when no segments are stored.
//!
//! Both modes use the spawning path — an export with no daemon should
//! start one rather than fail.

use crate::args::{CaptionFormat, ExportArgs};
use phoneme_core::backup::{self, RecordingChapters};
use phoneme_core::{
    Chapter, Config, ListFilter, MeetingDigest, PeriodDigest, Recording, Tag, TranscriptSegment,
};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

pub async fn run(args: ExportArgs, cfg: &Config) -> ExitCode {
    // Caption path: --captions present → fetch segments, emit SRT/VTT.
    if let Some(ref id_str) = args.captions {
        return run_captions(id_str, args.format, args.out.as_deref(), cfg).await;
    }

    // Library-zip path: requires the positional output argument.
    let zip_path = match args.output {
        Some(ref p) => p.clone(),
        None => {
            eprintln!(
                "error: an output file path is required when --captions is not set\n\
                 usage: phoneme export <FILE>  or  phoneme export --captions <ID>"
            );
            return ExitCode::from(crate::exit::GENERIC_FAIL);
        }
    };

    run_zip(&zip_path, cfg).await
}

// ── caption export ─────────────────────────────────────────────────────────────

async fn run_captions(
    id_str: &str,
    format: CaptionFormat,
    out_path: Option<&str>,
    cfg: &Config,
) -> ExitCode {
    let id = match phoneme_core::RecordingId::parse(id_str) {
        Some(id) => id,
        None => {
            eprintln!("error: '{}' is not a valid recording id", id_str);
            return ExitCode::FAILURE;
        }
    };

    let mut conn = match crate::client::Client::connect(cfg).await {
        Ok(c) => c,
        Err(e) => return e,
    };

    let value = match conn
        .send(phoneme_ipc::Request::GetSegments { id, variant: None })
        .await
    {
        Ok(v) => v,
        Err(e) => return e,
    };

    let segments: Vec<TranscriptSegment> = match serde_json::from_value(value) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: parsing segments response: {e}");
            return ExitCode::from(crate::exit::GENERIC_FAIL);
        }
    };

    if segments.is_empty() {
        eprintln!("no segments stored — retranscribe this recording to generate them");
        return ExitCode::from(crate::exit::NOT_FOUND);
    }

    let body = match format {
        CaptionFormat::Srt => phoneme_core::export::segments_to_srt(&segments),
        CaptionFormat::Vtt => phoneme_core::export::segments_to_vtt(&segments),
    };

    let ext = match format {
        CaptionFormat::Srt => "srt",
        CaptionFormat::Vtt => "vtt",
    };

    // Determine where to write: "-" → stdout, explicit path → file, else
    // default to `<recording-id>.<ext>` in the current directory.
    let dest = out_path
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}.{}", id_str, ext));

    if dest == "-" {
        if let Err(e) = std::io::stdout().write_all(body.as_bytes()) {
            eprintln!("error writing to stdout: {e}");
            return ExitCode::from(crate::exit::GENERIC_FAIL);
        }
    } else {
        match File::create(&dest) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(body.as_bytes()) {
                    eprintln!("error writing to {dest}: {e}");
                    return ExitCode::from(crate::exit::GENERIC_FAIL);
                }
            }
            Err(e) => {
                eprintln!("failed to create {dest}: {e}");
                return ExitCode::from(crate::exit::GENERIC_FAIL);
            }
        }
        println!("captions written to {dest}");
    }

    ExitCode::SUCCESS
}

// ── library zip export ─────────────────────────────────────────────────────────

async fn run_zip(zip_path: &str, cfg: &Config) -> ExitCode {
    let mut conn = match crate::client::Client::connect(cfg).await {
        Ok(c) => c,
        Err(e) => return e,
    };

    // Fetch the library + tags over IPC, then hand the typed values to the
    // shared backup writer (`phoneme_core::backup`) so the archive layout —
    // `catalog.json` + day-folder-preserving `audio/…` entries — has a single
    // home that `import-backup` reads back. Decoding into the typed DTOs here
    // (rather than re-zipping raw JSON) keeps export and restore in lock-step.
    let recordings_val = match conn
        .send(phoneme_ipc::Request::ListRecordings {
            filter: ListFilter::default(),
        })
        .await
    {
        Ok(val) => val,
        Err(e) => return e,
    };
    let recordings: Vec<Recording> = match serde_json::from_value(recordings_val) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to parse recordings response: {e}");
            return ExitCode::from(crate::exit::GENERIC_FAIL);
        }
    };

    // A tag-list failure is non-fatal: export the recordings + audio without
    // the tag catalog rather than aborting the whole backup.
    let tags: Vec<Tag> = conn
        .send(phoneme_ipc::Request::ListTags)
        .await
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // Whole-meeting digests live in their own side table (keyed by meeting_id),
    // so the per-recording list never carries them — fetch them separately so
    // they round-trip. Best-effort like the tags: a failure exports the rest.
    let meeting_digests: Vec<MeetingDigest> = conn
        .send(phoneme_ipc::Request::ListMeetingDigests)
        .await
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // Period digests (the date-window rollups) likewise live in their own side
    // table, so fetch them separately. Best-effort like the rest.
    let period_digests: Vec<PeriodDigest> = conn
        .send(phoneme_ipc::Request::ListPeriodDigests)
        .await
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // Which task/entity rows are user-added ('manual'), so restore can flip
    // them back after the setters re-insert everything as 'llm'. Best-effort:
    // an older daemon without the request just yields an empty list (matching
    // pre-fix backups).
    let manual_sources: Vec<backup::ManualSources> = conn
        .send(phoneme_ipc::Request::ManualSources)
        .await
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // Auto-generated chapters are a per-recording side table (keyed by id), so
    // there's no list request — fetch them one recording at a time over the same
    // client and keep only the recordings that have any. A failed fetch on one
    // recording warns and is skipped rather than aborting the whole backup,
    // matching the best-effort tag/digest handling above.
    let mut chapters: Vec<RecordingChapters> = Vec::new();
    for rec in &recordings {
        let value = match conn
            .send(phoneme_ipc::Request::GetChapters { id: rec.id.clone() })
            .await
        {
            Ok(v) => v,
            Err(_) => {
                eprintln!("warning: skipping chapters for {} (fetch failed)", rec.id);
                continue;
            }
        };
        let recs: Vec<Chapter> = match serde_json::from_value(value) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: skipping chapters for {} ({e})", rec.id);
                continue;
            }
        };
        if !recs.is_empty() {
            chapters.push(RecordingChapters {
                recording_id: rec.id.clone(),
                chapters: recs,
            });
        }
    }

    let expanded = match cfg.expanded() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(crate::exit::INVALID_CONFIG);
        }
    };
    let audio_dir = Path::new(&expanded.recording.audio_dir);

    if let Err(e) = backup::write_to_zip(
        &recordings,
        &tags,
        &meeting_digests,
        &period_digests,
        &chapters,
        &manual_sources,
        audio_dir,
        Path::new(zip_path),
    ) {
        eprintln!("failed to write backup zip: {e}");
        return ExitCode::from(crate::exit::GENERIC_FAIL);
    }

    println!("exported to {}", zip_path);
    ExitCode::SUCCESS
}

// The library-zip archive layout (day-folder-preserving entry names, the
// same-ms-different-day no-collision guarantee) now lives in — and is unit-
// tested by — `phoneme_core::backup`, which both this export and `import-backup`
// share. There is nothing zip-format-specific left to test here.
