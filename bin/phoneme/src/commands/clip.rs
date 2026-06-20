//! `phoneme clip <ID> <START> <END> [OUT]` — export a time range of a
//! recording's audio to a new WAV.
//!
//! Spawning path (it writes a file). START/END are seconds as floats (e.g.
//! `12.5`); they're converted to milliseconds and sent as one `ExportClip`
//! request. The daemon looks up the recording's audio path, slices the
//! `[start, end)` range on sample-frame boundaries, and writes a WAV with the
//! source's format — `end` is clamped to the recording's duration. When `OUT` is
//! omitted the clip lands next to the source with a `_clip_<start>-<end>` suffix.
//! Prints the path written.

use crate::args::ClipArgs;
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: ClipArgs, cfg: &Config, json: bool) -> ExitCode {
    let id = match RecordingId::parse(args.id.as_str()) {
        Some(id) => id,
        None => {
            eprintln!("error: '{}' is not a valid recording id", args.id);
            return ExitCode::FAILURE;
        }
    };

    // Validate the seconds up front so an obvious mistake fails before we spawn
    // a daemon or open a connection. The daemon re-validates the ms range too.
    if !(args.start.is_finite() && args.end.is_finite()) || args.start < 0.0 || args.end < 0.0 {
        eprintln!("error: start and end must be non-negative seconds");
        return ExitCode::FAILURE;
    }
    if args.start >= args.end {
        eprintln!(
            "error: start ({}) must be before end ({})",
            args.start, args.end
        );
        return ExitCode::FAILURE;
    }

    let start_ms = (args.start * 1000.0).round() as i64;
    let end_ms = (args.end * 1000.0).round() as i64;
    // Two distinct seconds can round to the same millisecond (e.g. 0.9995 and
    // 1.0004 both -> 1000). Catch it locally with an accurate message rather than
    // letting the daemon reject it with a misleading "start must be before end".
    if start_ms >= end_ms {
        eprintln!(
            "error: start and end round to the same millisecond ({start_ms}ms); use a wider range"
        );
        return ExitCode::FAILURE;
    }

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let value = match client
        .send(Request::ExportClip {
            id,
            start_ms,
            end_ms,
            out_path: args.out,
        })
        .await
    {
        Ok(v) => v,
        Err(code) => return code,
    };

    if json {
        output::print_json(&value);
        return ExitCode::SUCCESS;
    }

    let path = value.get("path").and_then(|p| p.as_str()).unwrap_or("");
    println!("wrote {path}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    async fn run_clip(args: ClipArgs, written: &str) -> (ExitCode, Vec<Request>) {
        let written = written.to_string();
        let mock = MockDaemon::spawn("clip", move |_req| {
            Response::Ok(serde_json::json!({ "path": written }))
        });
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code = tokio::time::timeout(Duration::from_secs(5), run(args, &cfg, false))
            .await
            .expect("clip must return promptly");
        (code, mock.received())
    }

    #[tokio::test]
    async fn converts_seconds_to_ms_and_forwards() {
        let id = RecordingId::new();
        let args = ClipArgs {
            id: id.to_string(),
            start: 12.5,
            end: 30.0,
            out: None,
        };
        let (code, reqs) = run_clip(args, "C:/audio/rec_clip_12500-30000.wav").await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::ExportClip {
                id,
                start_ms: 12_500,
                end_ms: 30_000,
                out_path: None,
            }]
        );
    }

    #[tokio::test]
    async fn forwards_explicit_out_path() {
        let id = RecordingId::new();
        let args = ClipArgs {
            id: id.to_string(),
            start: 0.0,
            end: 1.0,
            out: Some("C:/tmp/cut.wav".into()),
        };
        let (_code, reqs) = run_clip(args, "C:/tmp/cut.wav").await;
        assert_eq!(
            reqs,
            vec![Request::ExportClip {
                id,
                start_ms: 0,
                end_ms: 1_000,
                out_path: Some("C:/tmp/cut.wav".into()),
            }]
        );
    }

    #[tokio::test]
    async fn rejects_a_bad_recording_id() {
        let args = ClipArgs {
            id: "not-an-id".into(),
            start: 0.0,
            end: 1.0,
            out: None,
        };
        // No daemon needed: the id is rejected before connecting.
        let cfg = Config::default();
        let code = run(args, &cfg, false).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }

    #[tokio::test]
    async fn rejects_start_at_or_after_end() {
        let id = RecordingId::new();
        let args = ClipArgs {
            id: id.to_string(),
            start: 5.0,
            end: 5.0,
            out: None,
        };
        let cfg = Config::default();
        let code = run(args, &cfg, false).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }
}
