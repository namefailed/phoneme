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
            // Observe-only: if the daemon isn't running, "not reachable" is
            // the correct answer — no point spawning one just to ask its status.
            let mut client = match Client::connect_observe(cfg).await {
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
    if wait_for_pipe_death(pipe_name, STOP_WAIT).await {
        println!("daemon stopped");
        return ExitCode::SUCCESS;
    }
    println!("shutdown requested (daemon is still winding down)");
    ExitCode::SUCCESS
}

/// Poll until the daemon's pipe stops accepting connections, bounded by
/// `timeout`. Returns `true` once the pipe is gone (the daemon has exited),
/// `false` if it is still answering at the deadline. Shared by `daemon stop`
/// and `doctor --rebuild-catalog`, which must not touch the catalog files
/// while the daemon still holds them.
pub(crate) async fn wait_for_pipe_death(pipe_name: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if NamedPipeTransport::connect(pipe_name).await.is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no daemon listening, the wait reports "gone" on the first probe —
    /// well inside the timeout — so callers like `doctor --rebuild-catalog`
    /// never stall on an already-stopped daemon.
    #[tokio::test]
    async fn wait_for_pipe_death_returns_immediately_when_no_pipe_exists() {
        let started = std::time::Instant::now();
        let gone =
            wait_for_pipe_death("phoneme-test-no-such-pipe-m18", Duration::from_secs(5)).await;
        assert!(gone, "a non-existent pipe counts as dead");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the first failed probe should settle it, not the timeout"
        );
    }
}
