use crate::args::RecordArgs;
use crate::client::Client;
use crate::exit;
use futures::StreamExt;
use phoneme_core::{Config, RecordMode};
use phoneme_ipc::{DaemonEvent, Request};
use std::process::ExitCode;

pub async fn run(args: RecordArgs, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    // Non-blocking variants first.
    if args.start {
        return single_request(
            &mut client,
            Request::RecordStart {
                mode: RecordMode::Hold,
            },
            json,
        )
        .await;
    }
    if args.stop {
        return single_request(&mut client, Request::RecordStop, json).await;
    }
    if args.cancel {
        return single_request(&mut client, Request::RecordCancel, json).await;
    }

    // Oneshot / Duration / Hold-via-stdin all block on the event stream.
    let mode = if args.oneshot {
        RecordMode::Oneshot
    } else if let Some(secs) = args.duration {
        RecordMode::Duration { secs }
    } else {
        RecordMode::Hold
    };

    if let Err(code) = client.send_silent(Request::RecordStart { mode }).await {
        return code;
    }

    if matches!(mode, RecordMode::Hold) {
        // Wait for the user to hit Enter or close stdin.
        use tokio::io::{AsyncBufReadExt, BufReader};
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        let _ = reader.read_line(&mut line).await;
        if let Err(code) = client.send_silent(Request::RecordStop).await {
            return code;
        }
    }

    // Subscribe to events and wait for TranscriptionDone or *Failed.
    let mut events = match client.subscribe().await {
        Ok(s) => s,
        Err(code) => return code,
    };

    let timeout = std::time::Duration::from_secs(cfg.whisper.timeout_secs + 60);
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        match tokio::time::timeout(std::time::Duration::from_millis(500), events.next()).await {
            Ok(Some(Ok(DaemonEvent::TranscriptionDone { transcript, .. }))) => {
                if json {
                    crate::output::print_json(&serde_json::json!({ "transcript": transcript }));
                } else {
                    println!("{transcript}");
                }
                return ExitCode::SUCCESS;
            }
            Ok(Some(Ok(DaemonEvent::TranscriptionFailed { error, .. }))) => {
                eprintln!("transcription failed: {error}");
                return ExitCode::from(exit::Whisper_UNREACHABLE);
            }
            Ok(Some(Ok(_))) => continue, // other events
            Ok(Some(Err(e))) => {
                eprintln!("event stream error: {e}");
                return ExitCode::from(exit::DAEMON_NOT_REACHABLE);
            }
            Ok(None) => break,
            Err(_) => continue, // timeout slice; keep polling
        }
    }

    eprintln!("timed out waiting for transcription");
    ExitCode::from(exit::GENERIC_FAIL)
}

async fn single_request(client: &mut Client, req: Request, json: bool) -> ExitCode {
    match client.send(req).await {
        Ok(value) => {
            if json {
                crate::output::print_json(&value);
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
