use crate::args::ExportArgs;
use phoneme_core::{Config, ListFilter};
use std::fs::File;
use std::io::{Read, Write};
use std::process::ExitCode;
use zip::write::SimpleFileOptions;

pub async fn run(args: ExportArgs, cfg: &Config) -> ExitCode {
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

    let json_bytes = serde_json::to_vec_pretty(&export_data).unwrap();

    let file = match File::create(&args.output) {
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
    let _ = zip.write_all(&json_bytes);

    let expanded = match cfg.expanded() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(crate::exit::INVALID_CONFIG);
        }
    };

    let audio_dir = std::path::Path::new(&expanded.recording.audio_dir);
    if audio_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(audio_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let file_name = path.file_name().unwrap().to_string_lossy();
                    if file_name.ends_with(".wav") {
                        if let Err(e) = zip.start_file(format!("audio/{}", file_name), options) {
                            eprintln!("failed to write {file_name} to zip: {e}");
                            continue;
                        }
                        if let Ok(mut f) = File::open(&path) {
                            let mut buf = Vec::new();
                            let _ = f.read_to_end(&mut buf);
                            let _ = zip.write_all(&buf);
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

    println!("exported to {}", args.output);
    ExitCode::SUCCESS
}
