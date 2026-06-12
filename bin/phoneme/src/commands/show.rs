use crate::args::ShowArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Config, Recording, RecordingId, TranscriptSegment};
use phoneme_ipc::Request;
use std::process::ExitCode;

/// `start_ms` → `m:ss` (or `h:mm:ss` past an hour) for the timeline output.
fn fmt_ms(ms: i64) -> String {
    let total_secs = ms / 1000;
    let (h, m, s) = (total_secs / 3600, (total_secs % 3600) / 60, total_secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub async fn run(args: ShowArgs, cfg: &Config, json: bool) -> ExitCode {
    let id = match RecordingId::parse(args.id.as_str()) {
        Some(id) => id,
        None => {
            eprintln!("error: '{}' is not a valid recording id", args.id);
            return ExitCode::FAILURE;
        }
    };
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    if args.segments {
        let value = match client.send(Request::GetSegments { id }).await {
            Ok(v) => v,
            Err(code) => return code,
        };
        if json {
            output::print_json(&value);
            return ExitCode::SUCCESS;
        }
        let segments: Vec<TranscriptSegment> = match serde_json::from_value(value) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: parsing segments response: {e}");
                return ExitCode::from(exit::GENERIC_FAIL);
            }
        };
        if segments.is_empty() {
            // Normal for recordings transcribed before segment capture, or for
            // providers that return no timing data — say so instead of printing
            // nothing.
            eprintln!("no segments stored for this recording (retranscribe to capture them)");
            return ExitCode::from(exit::NOT_FOUND);
        }
        for seg in segments {
            let speaker = seg
                .speaker
                .map(|s| format!(" [Speaker {s}]"))
                .unwrap_or_default();
            println!(
                "{:>8}–{:<8}{} {}",
                fmt_ms(seg.start_ms),
                fmt_ms(seg.end_ms),
                speaker,
                seg.text
            );
        }
        return ExitCode::SUCCESS;
    }

    // Transcript variants fetch a single string from a dedicated request.
    if args.original || args.unedited {
        let req = if args.original {
            Request::GetOriginalTranscript { id }
        } else {
            Request::GetCleanTranscript { id }
        };
        let value = match client.send(req).await {
            Ok(v) => v,
            Err(code) => return code,
        };
        // The daemon returns either a string or null when no variant exists.
        let text = value.as_str().unwrap_or_default();
        if json {
            output::print_json(&value);
        } else if text.is_empty() {
            eprintln!("error: no transcript variant available for this recording");
            return ExitCode::from(exit::NOT_FOUND);
        } else {
            println!("{text}");
        }
        return ExitCode::SUCCESS;
    }

    let value = match client.send(Request::GetRecording { id }).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let row: Recording = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: parsing show response: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };
    if args.audio_path_only {
        println!("{}", row.audio_path);
    } else if json {
        output::print_json(&serde_json::to_value(row).unwrap_or_default());
    } else {
        output::print_recording_pretty(&row);
    }
    ExitCode::SUCCESS
}
