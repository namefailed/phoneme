//! `phoneme export` — two export modes behind one command.
//!
//! **Library zip** (`phoneme export <FILE>`): fetches every recording
//! (`ListRecordings` with the default filter) and the tag list
//! (`ListTags`), writes them as `catalog.json` (versioned envelope), and
//! packs every `.wav` under the configured audio dir into `audio/` —
//! a portable backup of the whole library.
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
use phoneme_core::{Config, ListFilter, TranscriptSegment};
use std::fs::File;
use std::io::{Read, Write};
use std::process::ExitCode;
use zip::write::SimpleFileOptions;

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

    let value = match conn.send(phoneme_ipc::Request::GetSegments { id }).await {
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

/// Zip-entry name for one audio file, preserving its day folder.
///
/// Recordings live at `<audio_dir>/<YYYY-MM-DD>/<HHmmssMMM>.wav` and the stem is
/// time-of-day only, so two recordings at the same ms-of-day on different days
/// share a stem. Naming the entry from the path RELATIVE to `audio_dir` (with
/// backslashes normalized to `/` for a portable archive) keeps the day folder,
/// so the two never collide to one entry. Falls back to the bare filename if the
/// path isn't under `audio_dir` (shouldn't happen — every path came from walking
/// it — but better a flat name than a dropped file).
fn audio_entry_name(audio_dir: &std::path::Path, path: &std::path::Path) -> String {
    match path.strip_prefix(audio_dir) {
        Ok(rel) => format!("audio/{}", rel.to_string_lossy().replace('\\', "/")),
        Err(_) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("audio/{name}")
        }
    }
}

async fn run_zip(zip_path: &str, cfg: &Config) -> ExitCode {
    let mut conn = match crate::client::Client::connect(cfg).await {
        Ok(c) => c,
        Err(e) => return e,
    };

    let recordings = match conn
        .send(phoneme_ipc::Request::ListRecordings {
            filter: ListFilter::default(),
        })
        .await
    {
        Ok(val) => val,
        Err(e) => return e,
    };

    let tags = match conn.send(phoneme_ipc::Request::ListTags).await {
        Ok(val) => val,
        Err(_) => serde_json::json!([]),
    };

    let export_data = serde_json::json!({
        "version": 1,
        "recordings": recordings,
        "tags": tags,
    });

    let json_bytes = match serde_json::to_vec_pretty(&export_data) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to serialize export data: {e}");
            return ExitCode::from(crate::exit::GENERIC_FAIL);
        }
    };

    let file = match File::create(zip_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("failed to create output file: {e}");
            return ExitCode::from(crate::exit::GENERIC_FAIL);
        }
    };

    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    if let Err(e) = zip.start_file("catalog.json", options) {
        eprintln!("failed to write catalog.json to zip: {e}");
        return ExitCode::from(crate::exit::GENERIC_FAIL);
    }
    if let Err(e) = zip.write_all(&json_bytes) {
        eprintln!("failed to write catalog bytes to zip: {e}");
        return ExitCode::from(crate::exit::GENERIC_FAIL);
    }

    let expanded = match cfg.expanded() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(crate::exit::INVALID_CONFIG);
        }
    };

    let audio_dir = std::path::Path::new(&expanded.recording.audio_dir);
    if audio_dir.exists() {
        let mut stack = vec![audio_dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.is_file() {
                        if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                            if file_name.ends_with(".wav") {
                                // Entry name preserves the day folder so two same-
                                // ms-different-day recordings don't collide — see
                                // `audio_entry_name`.
                                let entry_name = audio_entry_name(audio_dir, &path);
                                if let Err(e) = zip.start_file(entry_name, options) {
                                    eprintln!("failed to write {file_name} to zip: {e}");
                                    continue;
                                }
                                if let Ok(mut f) = File::open(&path) {
                                    let mut buf = Vec::new();
                                    if let Err(e) = f.read_to_end(&mut buf) {
                                        eprintln!("failed to read {file_name}: {e}");
                                        continue;
                                    }
                                    if let Err(e) = zip.write_all(&buf) {
                                        eprintln!("failed to write {file_name} bytes to zip: {e}");
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Err(e) = zip.finish() {
        eprintln!("failed to finalize zip: {e}");
        return ExitCode::from(crate::exit::GENERIC_FAIL);
    }

    println!("exported to {}", zip_path);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::audio_entry_name;
    use std::path::Path;

    #[test]
    fn entry_name_preserves_the_day_folder() {
        let audio_dir = Path::new("/data/audio");
        let path = Path::new("/data/audio/2026-05-19/143500042.wav");
        assert_eq!(
            audio_entry_name(audio_dir, path),
            "audio/2026-05-19/143500042.wav"
        );
    }

    #[test]
    fn same_ms_different_day_files_get_distinct_entries() {
        // The H1 data-loss case: two recordings at the same ms-of-day on
        // different days share a `143500042.wav` stem. The bare-filename naming
        // collapsed them to one `audio/143500042.wav` entry and the second
        // clobbered the first on restore. Preserving the day folder keeps both.
        let audio_dir = Path::new("/data/audio");
        let day1 = Path::new("/data/audio/2026-05-19/143500042.wav");
        let day2 = Path::new("/data/audio/2026-05-20/143500042.wav");
        let a = audio_entry_name(audio_dir, day1);
        let b = audio_entry_name(audio_dir, day2);
        assert_ne!(a, b, "same-ms-different-day files must not collide");
        assert_eq!(a, "audio/2026-05-19/143500042.wav");
        assert_eq!(b, "audio/2026-05-20/143500042.wav");
    }

    #[test]
    fn windows_separators_are_normalized_to_forward_slashes() {
        // Zip entry names use `/` regardless of host so the archive is portable;
        // strip_prefix on Windows yields `2026-05-19\143500042.wav`.
        let audio_dir = Path::new(r"C:\data\audio");
        let path = Path::new(r"C:\data\audio\2026-05-19\143500042.wav");
        let name = audio_entry_name(audio_dir, path);
        assert!(!name.contains('\\'), "no backslashes in entry name: {name}");
        assert_eq!(name, "audio/2026-05-19/143500042.wav");
    }

    #[test]
    fn path_outside_audio_dir_falls_back_to_flat_name() {
        let audio_dir = Path::new("/data/audio");
        let path = Path::new("/elsewhere/143500042.wav");
        assert_eq!(audio_entry_name(audio_dir, path), "audio/143500042.wav");
    }
}
