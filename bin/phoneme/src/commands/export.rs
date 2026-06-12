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
                                if let Err(e) =
                                    zip.start_file(format!("audio/{}", file_name), options)
                                {
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
