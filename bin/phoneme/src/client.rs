//! Thin IPC client wrapper.
//!
//! Resolves the pipe name from config (or default), connects with retry,
//! and exposes `send_request` returning a Result<serde_json::Value, ExitCode>.
//!
//! ## Spawning behaviour
//!
//! Two connection variants exist:
//!
//! - [`Client::connect`] — the **spawning** path used by commands that create
//!   work (record, import, retranscribe, …). If the daemon is not running it is
//!   started automatically before the request is sent.
//! - [`Client::connect_observe`] — the **observe-only** path used by read-only
//!   or inspection commands (`status`, `doctor`, `list`, `show`, `search`,
//!   `queue`, `watch`, …). If the daemon is not running the command fails
//!   immediately with a clear message instead of silently starting one — a
//!   daemon-is-down state is itself useful diagnostic information for those
//!   commands, and there is nothing to observe without one.

use crate::auto_spawn;
use crate::exit;
use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport, PROTOCOL_VERSION};
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
    ///
    /// Use this for commands that create work (`record`, `import`,
    /// `retranscribe`, `cleanup`, `summarize`, `reembed`, `refire-hook`,
    /// `delete`, `edit`, `notes`, `queue pause/resume/cancel/reorder`, `tag
    /// attach/detach/add/update/delete`, `profile use`, `hook test`, `export
    /// --captions`, `config reload/set`, `meeting start/stop/toggle`). These
    /// commands need a daemon to be running; starting one automatically on
    /// their behalf is the right behavior.
    ///
    /// For read-only or inspection commands use [`Client::connect_observe`].
    pub async fn connect(cfg: &Config) -> Result<Self, ExitCode> {
        let pipe_name = &cfg.daemon.pipe_name;
        let transport = match NamedPipeTransport::connect(pipe_name).await {
            Ok(t) => t,
            Err(_) => {
                if let Err(e) = auto_spawn::ensure_running(cfg).await {
                    eprintln!("error: failed to auto-spawn daemon: {e}");
                    return Err(ExitCode::from(exit::DAEMON_NOT_REACHABLE));
                }
                NamedPipeTransport::connect(pipe_name).await.map_err(|e| {
                    eprintln!("error: daemon not reachable: {e}");
                    ExitCode::from(exit::DAEMON_NOT_REACHABLE)
                })?
            }
        };
        let mut client = Self { transport };
        client.verify_protocol().await?;
        Ok(client)
    }

    /// Connect to the daemon without spawning it if absent.
    ///
    /// Use this for read-only or inspection commands — `status`, `doctor`,
    /// `list`, `show`, `search`, `queue list/counts/status`, `watch`, `tag
    /// list/for/usage`, `profile list`, `meeting tracks` — where the daemon
    /// not running is itself the answer rather than a fixable obstacle.
    /// `queue skip` rides this path too: it mutates, but only a live daemon
    /// mid-stage has anything to skip, so spawning one would mask reality. A
    /// clear error is printed and [`exit::DAEMON_NOT_REACHABLE`] is returned
    /// when the daemon is unreachable, letting the caller surface the fact
    /// that the daemon is down without masking it with a silent start.
    pub async fn connect_observe(cfg: &Config) -> Result<Self, ExitCode> {
        let pipe_name = &cfg.daemon.pipe_name;
        let transport = NamedPipeTransport::connect(pipe_name).await.map_err(|e| {
            eprintln!(
                "error: daemon not reachable: {e}\n\
                 hint: start it with `phoneme daemon start`"
            );
            ExitCode::from(exit::DAEMON_NOT_REACHABLE)
        })?;
        let mut client = Self { transport };
        client.verify_protocol().await?;
        Ok(client)
    }

    /// Best-effort IPC wire-protocol check (F3): refuse to operate against a
    /// daemon that reports an INCOMPATIBLE protocol, with a clear message. Only a
    /// daemon explicitly answering `compatible: false` is a hard stop — an old
    /// daemon that predates the handshake (it replies with an error), or any
    /// transport hiccup here, is treated as "unversioned, proceed" so a minor
    /// skew never bricks the CLI. A handshake is one request/response and leaves
    /// the connection in request mode (a later `subscribe` reframes it cleanly).
    async fn verify_protocol(&mut self) -> Result<(), ExitCode> {
        let resp = self
            .transport
            .request(Request::Handshake {
                protocol_version: PROTOCOL_VERSION,
            })
            .await;
        if let Ok(Response::Ok(v)) = resp {
            if v.get("compatible").and_then(|c| c.as_bool()) == Some(false) {
                let daemon_proto = v
                    .get("protocol_version")
                    .and_then(|p| p.as_u64())
                    .unwrap_or(0);
                let daemon_app = v.get("app_version").and_then(|s| s.as_str()).unwrap_or("?");
                eprintln!(
                    "error: this `phoneme` CLI speaks IPC protocol v{PROTOCOL_VERSION}, but the \
                     running daemon speaks v{daemon_proto} (daemon {daemon_app}). They're \
                     incompatible — run `phoneme daemon restart` (or update the CLI) so both are \
                     the same build."
                );
                return Err(ExitCode::from(exit::DAEMON_NOT_REACHABLE));
            }
        }
        Ok(())
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
