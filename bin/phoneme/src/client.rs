//! Thin IPC client wrapper.
//!
//! Resolves the pipe name from config (or default), connects with retry,
//! and exposes `send_request` returning a Result<serde_json::Value, ExitCode>.

use crate::auto_spawn;
use crate::exit;
use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};
use std::process::ExitCode;

// All four methods on Client are exercised once Tasks 5–11 wire up the
// individual subcommand handlers; until then clippy would otherwise warn
// that the struct and methods are dead.
#[allow(dead_code)]
pub struct Client {
    transport: NamedPipeTransport,
}

#[allow(dead_code)]
impl Client {
    /// Connect to the daemon. If absent, auto-spawn and retry. Returns an
    /// ExitCode if we ultimately can't reach the daemon.
    pub async fn connect(cfg: &Config) -> Result<Self, ExitCode> {
        let pipe_name = &cfg.daemon.pipe_name;
        match NamedPipeTransport::connect(pipe_name).await {
            Ok(t) => Ok(Self { transport: t }),
            Err(_) => {
                if let Err(e) = auto_spawn::ensure_running(cfg).await {
                    eprintln!("error: failed to auto-spawn daemon: {e}");
                    return Err(ExitCode::from(exit::DAEMON_NOT_REACHABLE));
                }
                NamedPipeTransport::connect(pipe_name)
                    .await
                    .map(|t| Self { transport: t })
                    .map_err(|e| {
                        eprintln!("error: daemon not reachable: {e}");
                        ExitCode::from(exit::DAEMON_NOT_REACHABLE)
                    })
            }
        }
    }

    /// Send a request and decode the response. On `Response::Err`, prints the
    /// error to stderr and returns the appropriate exit code.
    pub async fn send(&mut self, req: Request) -> Result<serde_json::Value, ExitCode> {
        match self.transport.request(req).await {
            Ok(Response::Ok(v)) => Ok(v),
            Ok(Response::Err(e)) => {
                eprintln!("error: {}", e.message);
                Err(ExitCode::from(exit::from_ipc_kind(e.kind)))
            }
            Err(e) => {
                eprintln!("error: transport: {e}");
                Err(ExitCode::from(exit::DAEMON_NOT_REACHABLE))
            }
        }
    }

    /// Send and ignore the response (for fire-and-forget requests).
    #[allow(dead_code)]
    pub async fn send_silent(&mut self, req: Request) -> Result<(), ExitCode> {
        self.send(req).await.map(|_| ())
    }

    /// Subscribe to events; useful for `--oneshot` waiting + `phoneme watch`.
    #[allow(dead_code)]
    pub async fn subscribe(
        &mut self,
    ) -> Result<
        futures::stream::BoxStream<'static, phoneme_ipc::TransportResult<phoneme_ipc::DaemonEvent>>,
        ExitCode,
    > {
        self.transport.subscribe().await.map_err(|e| {
            eprintln!("error: subscribe: {e}");
            ExitCode::from(exit::DAEMON_NOT_REACHABLE)
        })
    }
}
