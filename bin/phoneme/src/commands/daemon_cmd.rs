use crate::args::{DaemonAction, DaemonArgs};
use crate::auto_spawn;
use crate::client::Client;
use crate::exit;
use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};
use std::process::ExitCode;
use std::time::Duration;

/// How long `daemon stop` waits for the pipe to disappear after the daemon
/// acknowledges the Shutdown. The daemon finalizes an in-flight recording and
/// reaps its children on the way out, so a couple of seconds is normal.
const STOP_WAIT: Duration = Duration::from_secs(5);

pub async fn run(args: DaemonArgs, cfg: &Config, json: bool) -> ExitCode {
    match args.action.unwrap_or(DaemonAction::Status) {
        DaemonAction::Start => match auto_spawn::ensure_running(cfg).await {
            Ok(()) => {
                println!("daemon started");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(exit::GENERIC_FAIL)
            }
        },
        DaemonAction::Stop => stop(cfg).await,
        DaemonAction::Status => {
            let mut client = match Client::connect(cfg).await {
                Ok(c) => c,
                Err(code) => return code,
            };
            match client.send(Request::DaemonStatus).await {
                Ok(value) => {
                    if json {
                        crate::output::print_json(&value);
                    } else {
                        println!("running: {}", value["running"]);
                        println!("pid:     {}", value["pid"]);
                    }
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
    }
}

/// Graceful stop: send `Shutdown` over the pipe and wait (bounded) until the
/// daemon's pipe is gone. Connects DIRECTLY — never through `Client::connect`,
/// whose auto-spawn would briefly start a daemon just to stop it again.
/// Stopping a daemon that isn't running is a success, not an error.
async fn stop(cfg: &Config) -> ExitCode {
    let pipe_name = &cfg.daemon.pipe_name;
    let mut transport = match NamedPipeTransport::connect(pipe_name).await {
        Ok(t) => t,
        Err(_) => {
            println!("daemon is not running");
            return ExitCode::SUCCESS;
        }
    };
    match transport.request(Request::Shutdown).await {
        Ok(Response::Ok(_)) => {}
        Ok(Response::Err(e)) => {
            eprintln!("error: {}", e.message);
            return ExitCode::from(exit::from_ipc_kind(e.kind));
        }
        Err(e) => {
            eprintln!("error: transport: {e}");
            return ExitCode::from(exit::DAEMON_NOT_REACHABLE);
        }
    }
    drop(transport);

    // The daemon replies before it exits (finalizing any in-flight recording
    // and reaping its children on the way) — poll until the pipe vanishes so
    // "stopped" means stopped, not merely "asked".
    let deadline = std::time::Instant::now() + STOP_WAIT;
    while std::time::Instant::now() < deadline {
        if NamedPipeTransport::connect(pipe_name).await.is_err() {
            println!("daemon stopped");
            return ExitCode::SUCCESS;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    println!("shutdown requested (daemon is still winding down)");
    ExitCode::SUCCESS
}
