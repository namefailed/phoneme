//! `phoneme record` — push-to-talk recording from the terminal.
//!
//! Spawning path (`Client::connect`): recording is the daemon's reason to
//! exist, so a missing daemon is started. Two shapes:
//!
//! - **Non-blocking** (`record start` / `stop` / `toggle` / `cancel` /
//!   `pause` / `resume`): send one request (`RecordStart` / `RecordStop` /
//!   `RecordToggle` / `RecordCancel` / `RecordPause` / `RecordResume`) and
//!   exit 0 — hotkey/script bindings. `toggle` is atomic on the daemon side,
//!   and `start`/`toggle` take `--in-place` so a binding can start dictation.
//! - **Blocking** (default hold mode, `--oneshot`, `--duration N`): open an
//!   event subscription before doing anything else (a fast transcription can
//!   finish in the gap between stop and a late subscribe, and events are never
//!   replayed), then send `RecordStart` on a second connection, stop on
//!   Enter/EOF for hold mode, and wait for this recording's `TranscriptionDone`
//!   (print the transcript, exit 0) or `TranscriptionFailed` (exit 4). Other
//!   recordings' completions on the shared stream are filtered out by id.

use crate::args::{RecordAction, RecordArgs};
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

    // Preferred form: the non-blocking control subcommand
    // (`record start|stop|toggle|cancel|pause|resume`), consistent with
    // `meeting`, `daemon`, and the rest of the CLI. `start`/`toggle` carry their
    // own `--in-place` and `--recipe` so a binding can start dictation through a
    // chosen Playbook recipe.
    if let Some(action) = &args.action {
        let req = match action {
            RecordAction::Start { in_place, recipe } => {
                let recipe_id = match resolve_recipe(cfg, recipe.as_deref()) {
                    Ok(id) => id,
                    Err(code) => return code,
                };
                Request::RecordStart {
                    mode: RecordMode::Hold,
                    in_place: *in_place,
                    recipe_id,
                    whisper_model: None,
                    source: None,
                }
            }
            RecordAction::Stop => Request::RecordStop,
            RecordAction::Toggle { in_place, recipe } => {
                let recipe_id = match resolve_recipe(cfg, recipe.as_deref()) {
                    Ok(id) => id,
                    Err(code) => return code,
                };
                Request::RecordToggle {
                    in_place: *in_place,
                    recipe_id,
                    whisper_model: None,
                    source: None,
                }
            }
            RecordAction::Cancel => Request::RecordCancel,
            RecordAction::Pause => Request::RecordPause,
            RecordAction::Resume => Request::RecordResume,
        };
        return single_request(&mut client, req, json).await;
    }

    // Blocking modes (oneshot / duration / hold-via-stdin) honor the top-level
    // `--recipe`. Resolve it to an id before opening the event subscription so a
    // bad value errors out without leaving a dangling connection.
    let blocking_recipe_id = match resolve_recipe(cfg, args.recipe.as_deref()) {
        Ok(id) => id,
        Err(code) => return code,
    };

    // Oneshot / Duration / Hold-via-stdin all block on the event stream.
    let mode = if args.oneshot {
        RecordMode::Oneshot
    } else if let Some(secs) = args.duration {
        RecordMode::Duration { secs }
    } else {
        RecordMode::Hold
    };

    // Subscribe before starting (and, for hold mode, before stopping) the
    // recording. The daemon only delivers events to subscriptions that exist at
    // emit time, and a fast transcription (the in-place fast lane especially)
    // can emit TranscriptionDone in the gap between RecordStop and a late
    // subscription — the CLI would then hang until its timeout waiting for an
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
            recipe_id: blocking_recipe_id,
            whisper_model: None,
            source: None,
        })
        .await
    {
        Ok(v) => v,
        Err(code) => return code,
    };
    // The subscription is open for the whole take, so completion events from
    // unrelated pipeline work (imports, retranscribes) can arrive too. Only
    // this recording's id may end the wait.
    let rec_id = started
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    if matches!(mode, RecordMode::Hold) {
        // Wait for the user to hit Enter / close stdin (normal stop) or press
        // Ctrl+C. Without the Ctrl+C arm an interrupt would tear the process
        // down while the daemon kept recording indefinitely, leaking the take.
        // On Ctrl+C we discard the in-progress recording (RecordCancel) and
        // exit; a clean Enter/EOF stops and keeps it (RecordStop).
        use tokio::io::{AsyncBufReadExt, BufReader};
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        tokio::select! {
            _ = reader.read_line(&mut line) => {
                if let Err(code) = control.send_silent(Request::RecordStop).await {
                    return code;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                let _ = control.send_silent(Request::RecordCancel).await;
                eprintln!("interrupted — discarded the in-progress recording");
                return ExitCode::from(exit::GENERIC_FAIL);
            }
        }
    }

    // Wait for this recording's TranscriptionDone or *Failed. In Duration mode
    // the daemon records for `secs` before it even starts transcribing, so add
    // the known capture length to the budget — otherwise a long `--duration`
    // false-times-out while the daemon is recording normally.
    let capture_secs = match mode {
        RecordMode::Duration { secs } => secs as u64,
        _ => 0,
    };
    let timeout =
        std::time::Duration::from_secs(cfg.whisper.timeout_secs + 60 + capture_secs);
    let start = std::time::Instant::now();

    // `rec_id` is `None` only if the daemon's RecordStart response carried no
    // id (it always does); in that case fall back to accepting any completion.
    let is_ours =
        |id: &phoneme_core::RecordingId| rec_id.as_deref().is_none_or(|r| r == id.to_string());

    while start.elapsed() < timeout {
        // Poll the event stream in 500ms slices, but also watch for Ctrl+C the
        // whole time. Hold mode handled its interrupt above; Oneshot/Duration
        // block here instead, and without this arm an interrupt would tear the
        // CLI down while the daemon kept recording — the same leak the Hold path
        // avoids. On Ctrl+C we discard the in-progress take and exit.
        let next = tokio::select! {
            r = tokio::time::timeout(std::time::Duration::from_millis(500), events.next()) => r,
            _ = tokio::signal::ctrl_c() => {
                let _ = control.send_silent(Request::RecordCancel).await;
                eprintln!("interrupted — discarded the in-progress recording");
                return ExitCode::from(exit::GENERIC_FAIL);
            }
        };
        match next {
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

/// Map an optional `--recipe ID|NAME` value to the IPC `recipe_id` field:
/// `None` (absent flag) stays `None` (the daemon's default pipeline); a present
/// value is resolved against `config.recipes` (id then name) to its stable id.
/// An unmatched value prints the available recipes and yields an error exit so
/// a typo never silently runs the default pipeline.
fn resolve_recipe(cfg: &Config, value: Option<&str>) -> Result<Option<String>, ExitCode> {
    match value {
        None => Ok(None),
        Some(v) => match crate::commands::recipe::resolve(cfg, v) {
            Ok(id) => Ok(Some(id)),
            Err(msg) => {
                eprintln!("error: {msg}");
                Err(ExitCode::from(exit::GENERIC_FAIL))
            }
        },
    }
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

    /// The blocking record path must have its event subscription open before
    /// the recording is started/stopped: a fast transcription completes right
    /// after the stop, and an event emitted before the subscription exists is
    /// never replayed, so the CLI hangs to its timeout. The mock daemon emits
    /// `TranscriptionDone` immediately after answering `RecordStart`, and only
    /// to a subscriber that already existed when the start arrived — exactly the
    /// window a subscribe-after-stop ordering would miss.
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
                                    // but never wait past sending the start
                                    // response: a subscribe-after-stop ordering
                                    // can't subscribe until this response is out,
                                    // so it could never satisfy this flag.
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
            action: None,
            oneshot: true,
            duration: None,
            in_place: false,
            recipe: None,
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

    /// An otherwise-empty `RecordArgs` carrying just the given subcommand — the
    /// `record <action>` form (the only way to issue a non-blocking control now).
    fn args_action(action: RecordAction) -> RecordArgs {
        RecordArgs {
            action: Some(action),
            oneshot: false,
            duration: None,
            in_place: false,
            recipe: None,
        }
    }

    /// Run `record <action>` against a mock daemon and assert the single request
    /// it sends. Covers the new subcommand dispatch for every non-blocking verb.
    async fn assert_action_sends(label: &str, action: RecordAction, expect: Request) {
        let mock = MockDaemon::spawn(label, |_req| {
            Response::Ok(serde_json::json!({ "id": RecordingId::new().to_string() }))
        });
        let mut cfg = phoneme_core::Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(args_action(action), &cfg, false),
        )
        .await
        .expect("a non-blocking record subcommand must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(mock.received(), vec![expect]);
    }

    #[tokio::test]
    async fn record_start_subcommand_sends_record_start() {
        assert_action_sends(
            "start",
            RecordAction::Start {
                in_place: false,
                recipe: None,
            },
            Request::RecordStart {
                mode: RecordMode::Hold,
                in_place: false,
                recipe_id: None,
                whisper_model: None,
                source: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn record_start_subcommand_carries_in_place() {
        assert_action_sends(
            "start-ip",
            RecordAction::Start {
                in_place: true,
                recipe: None,
            },
            Request::RecordStart {
                mode: RecordMode::Hold,
                in_place: true,
                recipe_id: None,
                whisper_model: None,
                source: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn record_start_subcommand_resolves_recipe_by_name() {
        // "Meeting notes" is the display name of the seeded `meeting_notes`
        // recipe; the subcommand must send the resolved id, not the typed name.
        assert_action_sends(
            "start-recipe",
            RecordAction::Start {
                in_place: false,
                recipe: Some("Meeting notes".into()),
            },
            Request::RecordStart {
                mode: RecordMode::Hold,
                in_place: false,
                recipe_id: Some("meeting_notes".into()),
                whisper_model: None,
                source: None,
            },
        )
        .await;
    }

    /// A bad `--recipe` on a non-blocking subcommand errors before connecting,
    /// so the daemon never sees a request.
    #[tokio::test]
    async fn record_start_subcommand_rejects_unknown_recipe() {
        let mock = MockDaemon::spawn("start-bad-recipe", |_req| {
            Response::Ok(serde_json::json!({ "id": RecordingId::new().to_string() }))
        });
        let mut cfg = phoneme_core::Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let action = RecordAction::Start {
            in_place: false,
            recipe: Some("no-such-recipe".into()),
        };
        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(args_action(action), &cfg, false),
        )
        .await
        .expect("must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
        assert!(
            mock.received().is_empty(),
            "a bad --recipe must not reach the daemon"
        );
    }

    #[tokio::test]
    async fn record_stop_subcommand_sends_record_stop() {
        assert_action_sends("stop", RecordAction::Stop, Request::RecordStop).await;
    }

    #[tokio::test]
    async fn record_toggle_subcommand_sends_record_toggle() {
        assert_action_sends(
            "toggle",
            RecordAction::Toggle {
                in_place: false,
                recipe: None,
            },
            Request::RecordToggle {
                in_place: false,
                recipe_id: None,
                whisper_model: None,
                source: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn record_cancel_subcommand_sends_record_cancel() {
        assert_action_sends("cancel", RecordAction::Cancel, Request::RecordCancel).await;
    }

    #[tokio::test]
    async fn record_pause_subcommand_sends_record_pause() {
        assert_action_sends("pause-sub", RecordAction::Pause, Request::RecordPause).await;
    }

    #[tokio::test]
    async fn record_resume_subcommand_sends_record_resume() {
        assert_action_sends("resume-sub", RecordAction::Resume, Request::RecordResume).await;
    }
}
