//! `phoneme import <FILE-OR-URL>` — feed audio into the pipeline.
//!
//! Two inputs:
//!  - a local audio file (wav/mp3/m4a/flac), or
//!  - an http(s) URL (e.g. a YouTube link), whose audio track is downloaded with
//!    yt-dlp into a temp dir and then imported like any local file.
//!
//! Either way the extension is pre-checked locally and the path canonicalized to
//! an absolute one — the daemon has its own working directory, so a relative path
//! would not resolve the same way on its side. Then `ImportRecording` is sent;
//! the daemon decodes the file into its own canonical WAV, catalogs, and enqueues
//! it exactly like a mic recording (so a downloaded temp file is safe to delete
//! once the call returns). Prints the new recording id; transcription progress is
//! observable via `phoneme watch` or `phoneme queue`.

use crate::args::{AudioFormat, ImportArgs};
use crate::client::Client;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

/// Extensions the daemon can decode. Kept in sync with
/// `phoneme_audio::SUPPORTED_EXTENSIONS`; duplicated here so the CLI doesn't
/// pull in the heavy audio/codec dependency just for a local pre-check (the
/// daemon validates authoritatively anyway).
const SUPPORTED_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "flac"];

fn is_supported(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => SUPPORTED_EXTENSIONS
            .iter()
            .any(|s| ext.eq_ignore_ascii_case(s)),
        None => false,
    }
}

/// Whether the import argument is a URL (download via yt-dlp) rather than a path.
fn is_url(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("http://") || s.starts_with("https://")
}

/// Build the yt-dlp invocation: prefer a `yt-dlp` binary on PATH, else fall back
/// to `python -m yt_dlp` (how pip installs it when its Scripts dir isn't on
/// PATH). `None` if neither is available.
fn yt_dlp_command() -> Option<Command> {
    if let Ok(p) = which::which("yt-dlp") {
        return Some(Command::new(p));
    }
    let py = which::which("python")
        .or_else(|_| which::which("python3"))
        .ok()?;
    let mut c = Command::new(py);
    c.args(["-m", "yt_dlp"]);
    Some(c)
}

/// Detect an installed JavaScript runtime for yt-dlp's YouTube extractor. Modern
/// YouTube needs one to resolve audio formats (without it yt-dlp warns and may
/// fail). Prefer deno (yt-dlp's default), then node / bun. `None` → let yt-dlp
/// try unaided.
fn js_runtime() -> Option<&'static str> {
    ["deno", "node", "bun"]
        .into_iter()
        .find(|rt| which::which(rt).is_ok())
}

/// Download the audio track of `url` via yt-dlp into a fresh temp dir, returning
/// the temp dir (keep it alive until the import call returns — dropping it
/// deletes the download) and the path to the extracted audio file.
fn download_audio(url: &str, format: AudioFormat) -> Result<(tempfile::TempDir, PathBuf), String> {
    let mut cmd = yt_dlp_command().ok_or_else(|| {
        "yt-dlp not found. Install it with:  python -m pip install -U yt-dlp\n\
         (ffmpeg must also be installed and on PATH for audio extraction)."
            .to_string()
    })?;

    let dir = tempfile::Builder::new()
        .prefix("phoneme-yt-")
        .tempdir()
        .map_err(|e| format!("could not create temp dir: {e}"))?;
    // Keep the title in the filename (truncated, sanitized by yt-dlp) so it can
    // serve as a sensible fallback recording title.
    let out_template = dir.path().join("%(title).80s [%(id)s].%(ext)s");

    cmd.arg("-x") // extract audio only
        .arg("--audio-format")
        .arg(format.as_str())
        .arg("--no-playlist") // a playlist URL imports just the one video
        // Bound the download so a stall or a hostile URL can't wedge the CLI or
        // fill the disk: abort a stalled connection, refuse an oversize file before
        // it lands (the daemon's import cap is 2 GiB), and --no-part keeps a single
        // clean output file so the post-download pick stays deterministic.
        .arg("--socket-timeout")
        .arg("30")
        .arg("--max-filesize")
        .arg("2G")
        .arg("--no-part")
        .arg("-o")
        .arg(&out_template);
    // Give YouTube's extractor a JS runtime when one is installed, otherwise
    // modern YouTube format resolution warns and can fail.
    if let Some(rt) = js_runtime() {
        cmd.arg("--js-runtimes").arg(rt);
    }
    // `--` ends option parsing so a URL can never be mistaken for a yt-dlp flag
    // (defense-in-depth; `is_url` already requires an http(s) prefix).
    cmd.arg("--").arg(url);
    // Wall-clock backstop: even past the per-socket timeout, a hung yt-dlp (or one
    // blocked on a prompt) can't hang the CLI forever — kill it after 15 min.
    let status = run_with_timeout(cmd, Duration::from_secs(900))?;
    if !status.success() {
        return Err(format!("yt-dlp failed ({status})"));
    }

    // After `-x`, the temp dir holds exactly the extracted audio file.
    let file = std::fs::read_dir(dir.path())
        .map_err(|e| format!("could not read download dir: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.is_file() && is_supported(p))
        .ok_or_else(|| "yt-dlp produced no supported audio file".to_string())?;
    Ok((dir, file))
}

/// Run `cmd` to completion or kill it after `timeout`. `std::process` has no
/// built-in timeout, so spawn and poll `try_wait` — a hung child (stalled network
/// past the socket timeout, or one blocked on a prompt) is killed rather than
/// hanging the CLI forever.
fn run_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let mut child = cmd.spawn().map_err(|e| format!("failed to run yt-dlp: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "yt-dlp timed out after {}s — aborted",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("failed to wait on yt-dlp: {e}")),
        }
    }
}

pub async fn run(args: ImportArgs, cfg: &Config) -> ExitCode {
    // For a URL, download first; hold the temp dir alive for the whole function
    // so the file survives until the daemon has decoded it (the import call
    // blocks until decode completes), then it's cleaned up on return.
    // Trim once: `is_url` trims internally for its prefix test, but the raw value
    // was being handed to yt-dlp / used as the path — a "  https://… " input then
    // failed confusingly. Use the trimmed value everywhere.
    let input = args.file.trim();
    let _tmp: Option<tempfile::TempDir>;
    let local_path: PathBuf = if is_url(input) {
        eprintln!("downloading audio from {input} …");
        match download_audio(input, args.format) {
            Ok((dir, file)) => {
                _tmp = Some(dir);
                file
            }
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        _tmp = None;
        PathBuf::from(input)
    };

    if !local_path.exists() {
        eprintln!("error: file not found: {}", local_path.display());
        return ExitCode::FAILURE;
    }
    if !is_supported(&local_path) {
        eprintln!(
            "error: unsupported audio format (supported: {})",
            SUPPORTED_EXTENSIONS.join(", ")
        );
        return ExitCode::FAILURE;
    }

    // Resolve to an absolute path: the daemon runs with its own working
    // directory, so a relative path supplied to the CLI would not resolve the
    // same way on its side.
    let abs = match std::fs::canonicalize(&local_path) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(e) => {
            eprintln!("error: could not resolve {}: {e}", local_path.display());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(is_url("https://www.youtube.com/watch?v=abc"));
        assert!(is_url("http://youtu.be/abc"));
        assert!(is_url("  https://example.com/x  "));
        assert!(!is_url("C:\\Users\\me\\clip.m4a"));
        assert!(!is_url("/home/me/clip.wav"));
        assert!(!is_url("clip.mp3"));
        assert!(!is_url("ftp://example.com/x"));
    }

    #[test]
    fn supported_extensions() {
        assert!(is_supported(Path::new("a.m4a")));
        assert!(is_supported(Path::new("a.WAV")));
        assert!(is_supported(Path::new("a.flac")));
        assert!(!is_supported(Path::new("a.webm")));
        assert!(!is_supported(Path::new("a.opus")));
        assert!(!is_supported(Path::new("noext")));
    }
}
