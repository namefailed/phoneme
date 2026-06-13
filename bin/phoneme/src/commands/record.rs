//! `phoneme record` — push-to-talk recording from the terminal.
//!
//! Spawning path (`Client::connect`): recording is the daemon's reason to
//! exist, so a missing daemon is started. Two shapes:
//!
//! - **Non-blocking** (`--start` / `--stop` / `--toggle` / `--cancel`): send
//!   one request (`RecordStart` / `RecordStop` / `RecordToggle` /
//!   `RecordCancel`) and exit 0 — hotkey/script bindings. `--toggle` is
//!   atomic on the daemon side, and `--in-place` rides along so a binding
//!   can start dictation.
//! - **Blocking** (default hold mode, `--oneshot`, `--duration N`): open an
//!   event subscription FIRST (a fast transcription can finish in the gap
//!   between stop and a late subscribe — events are never replayed), then
//!   send `RecordStart` on a second connection, stop on Enter/EOF for hold
//!   mode, and wait for THIS recording's `TranscriptionDone` (print the
//!   transcript, exit 0) or `TranscriptionFailed` (exit 4). Other
//!   recordings' completions on the shared stream are ignored by id.

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
                in_place: args.in_place,
            },
            json,
        )
        .await;
    }
    if args.stop {
        return single_request(&mut client, Request::RecordStop, json).await;
    }
    if args.toggle {
        // Atomic start-if-idle / stop-if-active, mirroring the GUI record
        // hotkey. Honors --in-place so a toggle binding can start an in-place
        // recording the same way `record --start --in-place` does.
        return single_request(
            &mut client,
            Request::RecordToggle {
                in_place: args.in_place,
            },
            json,
        )
        .await;
    }
    if args.cancel {
        return single_request(&mut client, Request::RecordCancel, json).await;
    }
    if args.pause {
        return single_request(&mut client, Request::RecordPause, json).await;
    }
    if args.resume {
        return single_request(&mut client, Request::RecordResume, json).await;
    }

    // Oneshot / Duration / Hold-via-stdin all block on the event stream.
    let mode = if args.oneshot {
        RecordMode::Oneshot
    } else if let Some(secs) = args.duration {
        RecordMode::Duration { secs }
    } else {
        RecordMode::Hold
    };

    // Subscribe BEFORE starting (and, for hold mode, before stopping) the
    // recording. The daemon only delivers events to subscriptions that exist
    // at emit time — a fast transcription (the in-place fast lane especially)
    // can emit TranscriptionDone in the gap between RecordStop and a late
    // subscription, and the CLI then hangs until its timeout waiting for an
    // event that already happened. Subscribing consumes this connection's
    // request channel, so the start/stop control requests ride a second
    // connection opened next.
    let mut events = match client.subscribe().await {
        Ok(s) => s,
        Err(code) => return code,
    };
    let mut control = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let started = match control
        .send(Request::RecordStart {
            mode,
            in_place: args.in_place,
        })
        .await
    {
        Ok(v) => v,
        Err(code) => return code,
    };
    // The subscription is now open for the whole take, so completion events
    // from unrelated pipeline work (imports, retranscribes) can arrive too —
    // only this recording's id may end the wait.
    let rec_id = started
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    if matches!(mode, RecordMode::Hold) {
        // Wait for the user to hit Enter or close stdin.
        use tokio::io::{AsyncBufReadExt, BufReader};
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        let _ = reader.read_line(&mut line).await;
        if let Err(code) = control.send_silent(Request::RecordStop).await {
            return code;
        }
    }

    // Wait for this recording's TranscriptionDone or *Failed.
    let timeout = std::time::Duration::from_secs(cfg.whisper.timeout_secs + 60);
    let start = std::time::Instant::now();

    // `None` only if the daemon's RecordStart response had no id (it always
    // does) — then fall back to accepting any completion, the old behavior.
    let is_ours =
        |id: &phoneme_core::RecordingId| rec_id.as_deref().is_none_or(|r| r == id.to_string());

    while start.elapsed() < timeout {
        match tokio::time::timeout(std::time::Duration::from_millis(500), events.next()).await {
            Ok(Some(Ok(DaemonEvent::TranscriptionDone { id, transcript }))) => {
                if !is_ours(&id) {
                    continue;
                }
                if json {
                    crate::output::print_json(&serde_json::json!({ "transcript": transcript }));
                } else {
                    println!("{transcript}");
                }
                return ExitCode::SUCCESS;
            }
            Ok(Some(Ok(DaemonEvent::TranscriptionFailed { id, error }))) => {
                if !is_ours(&id) {
                    continue;
                }
                eprintln!("transcription failed: {error}");
                return ExitCode::from(exit::WHISPER_UNREACHABLE);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::RecordArgs;
    use phoneme_core::RecordingId;
    use phoneme_ipc::{NamedPipeConnection, NamedPipeListener, Response, ServerRequest};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn unique_pipe(label: &str) -> String {
        let pid = std::process::id();
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("phoneme-record-test-{label}-{pid}-{ns}")
    }

    /// The blocking record path must have its event subscription open BEFORE
    /// the recording is started/stopped: a fast transcription completes right
    /// after the stop, and an event emitted before the subscription exists is
    /// never replayed — the CLI then hangs to its timeout. The mock daemon
    /// emits `TranscriptionDone` immediately after answering `RecordStart`,
    /// and only to a subscriber that ALREADY existed when the start arrived —
    /// exactly the window the old subscribe-after-stop ordering lost.
    #[tokio::test]
    async fn blocking_record_subscribes_before_starting() {
        let name = unique_pipe("order");
        let mut listener = NamedPipeListener::bind(&name).expect("bind mock daemon pipe");

        let rec_id = RecordingId::new();
        // The subscriber connection, parked by the mock when SubscribeEvents
        // arrives so the RecordStart handler can emit the completion on it.
        let subscriber: Arc<tokio::sync::Mutex<Option<NamedPipeConnection>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let subscribed_before_start = Arc::new(AtomicBool::new(false));

        let responder = {
            let subscriber = subscriber.clone();
            let subscribed_before_start = subscribed_before_start.clone();
            let rec_id = rec_id.clone();
            tokio::spawn(async move {
                loop {
                    let Ok(mut conn) = listener.accept().await else {
                        break;
                    };
                    let subscriber = subscriber.clone();
                    let subscribed_before_start = subscribed_before_start.clone();
                    let rec_id = rec_id.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(req)) = conn.recv().await {
                            let ServerRequest::Known(req) = req else {
                                continue;
                            };
                            match *req {
                                Request::SubscribeEvents => {
                                    // Park this connection as the event stream;
                                    // the subscribe protocol sends no Response.
                                    *subscriber.lock().await = Some(conn);
                                    return;
                                }
                                Request::RecordStart { .. } => {
                                    // Absorb in-flight scheduling (the client's
                                    // SubscribeEvents bytes may still be landing),
                                    // but never wait past the start RESPONSE:
                                    // the old buggy ordering can't subscribe
                                    // until this response is sent, so it can
                                    // never satisfy this flag.
                                    for _ in 0..100 {
                                        if subscriber.lock().await.is_some() {
                                            subscribed_before_start.store(true, Ordering::SeqCst);
                                            break;
                                        }
                                        tokio::time::sleep(Duration::from_millis(5)).await;
                                    }
                                    let res = Response::Ok(
                                        serde_json::json!({ "id": rec_id.to_string() }),
                                    );
                                    if conn.send_response(res).await.is_err() {
                                        return;
                                    }
                                    // The fast transcription: done the instant
                                    // the start was acknowledged.
                                    if let Some(sub) = subscriber.lock().await.as_mut() {
                                        let _ = sub
                                            .send_event(DaemonEvent::TranscriptionDone {
                                                id: rec_id.clone(),
                                                transcript: "fast transcript".into(),
                                            })
                                            .await;
                                    }
                                }
                                _ => {
                                    let res = Response::Ok(serde_json::Value::Null);
                                    if conn.send_response(res).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    });
                }
            })
        };

        let mut cfg = phoneme_core::Config::default();
        cfg.daemon.pipe_name = name;

        let args = RecordArgs {
            oneshot: true,
            duration: None,
            start: false,
            stop: false,
            toggle: false,
            cancel: false,
            pause: false,
            resume: false,
            in_place: false,
        };

        // Outer guard so a regression (event lost → CLI waits for its full
        // transcription timeout) fails fast instead of hanging the suite.
        let code = tokio::time::timeout(Duration::from_secs(10), run(args, &cfg, false))
            .await
            .expect("record must complete promptly when the completion event is delivered");
        responder.abort();

        assert!(
            subscribed_before_start.load(Ordering::SeqCst),
            "the event subscription must exist before RecordStart is sent"
        );
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::SUCCESS),
            "record must exit success after receiving TranscriptionDone"
        );
    }

    use crate::commands::test_support::MockDaemon;

    fn args_with(pause: bool, resume: bool) -> RecordArgs {
        RecordArgs {
            oneshot: false,
            duration: None,
            start: false,
            stop: false,
            toggle: false,
            cancel: false,
            pause,
            resume,
            in_place: false,
        }
    }

    /// `--pause` sends exactly `RecordPause` on the non-blocking path and exits
    /// success — it must not subscribe or block on the event stream.
    #[tokio::test]
    async fn record_pause_sends_record_pause() {
        let mock = MockDaemon::spawn("pause", |_req| {
            Response::Ok(serde_json::json!({ "id": RecordingId::new().to_string() }))
        });
        let mut cfg = phoneme_core::Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(args_with(true, false), &cfg, false),
        )
        .await
        .expect("--pause must return promptly without blocking");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(mock.received(), vec![Request::RecordPause]);
    }

    /// `--resume` sends exactly `RecordResume`.
    #[tokio::test]
    async fn record_resume_sends_record_resume() {
        let mock = MockDaemon::spawn("resume", |_req| {
            Response::Ok(serde_json::json!({ "id": RecordingId::new().to_string() }))
        });
        let mut cfg = phoneme_core::Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(args_with(false, true), &cfg, false),
        )
        .await
        .expect("--resume must return promptly without blocking");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(mock.received(), vec![Request::RecordResume]);
    }
}
