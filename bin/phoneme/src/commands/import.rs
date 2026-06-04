use crate::args::ImportArgs;
use crate::client::Client;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::path::Path;
use std::process::ExitCode;

/// Extensions the daemon can decode. Kept in sync with
/// `phoneme_audio::SUPPORTED_EXTENSIONS`; duplicated here so the CLI doesn't
/// pull in the heavy audio/codec dependency just for a local pre-check (the
/// daemon validates authoritatively anyway).
const SUPPORTED_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a"];

fn is_supported(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => SUPPORTED_EXTENSIONS
            .iter()
            .any(|s| ext.eq_ignore_ascii_case(s)),
        None => false,
    }
}

pub async fn run(args: ImportArgs, cfg: &Config) -> ExitCode {
    let path = Path::new(&args.file);
    if !path.exists() {
        eprintln!("error: file not found: {}", args.file);
        return ExitCode::FAILURE;
    }
    if !is_supported(path) {
        eprintln!(
            "error: unsupported audio format (supported: {})",
            SUPPORTED_EXTENSIONS.join(", ")
        );
        return ExitCode::FAILURE;
    }

    // Resolve to an absolute path: the daemon runs with its own working
    // directory, so a relative path supplied to the CLI would not resolve the
    // same way on its side.
    let abs = match std::fs::canonicalize(path) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(e) => {
            eprintln!("error: could not resolve {}: {e}", args.file);
            return ExitCode::FAILURE;
        }
    };

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.send(Request::ImportRecording { path: abs }).await {
        Ok(v) => {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
            println!("imported {id}");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
