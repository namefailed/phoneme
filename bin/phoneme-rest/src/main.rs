//! `phoneme-rest` — a localhost HTTP/REST + SSE bridge over the phoneme daemon.
//!
//! This binary is a **thin** front-end: every REST endpoint maps one HTTP call
//! to exactly one [`phoneme_ipc::Request`], forwards it to the running daemon
//! over the existing named pipe, and returns the daemon's JSON answer verbatim.
//! `GET /api/events` streams the daemon's [`phoneme_ipc::DaemonEvent`]
//! broadcast as Server-Sent Events. There is no business logic here — the
//! daemon remains the single source of truth.
//!
//! ## Security: loopback is the trust boundary
//!
//! The server binds **`127.0.0.1` only** — never `0.0.0.0`. Phoneme is a
//! local-first, single-user app; the daemon's IPC pipe is already owner-only
//! (see `phoneme-ipc`'s named-pipe ACL), and this bridge keeps that posture by
//! refusing to listen on any non-loopback interface. Anything reachable on
//! loopback can already drive the daemon through the CLI, so loopback is the
//! boundary; exposing this surface to a network would widen it. If you need
//! remote access, put an authenticating reverse proxy in front — do not change
//! the bind address.
//!
//! ## Opt-in
//!
//! The bridge is **off by default**. It reads `[rest_api]` from the active
//! config and refuses to start (clean message, non-zero exit) unless
//! `enabled = true`. See `docs/developer-guide/rest_api.md` and the
//! [`phoneme_core::config::RestApiConfig`] struct.

mod daemon;
mod error;
mod handlers;
mod request_map;
mod server;
mod sse;

use std::net::{Ipv4Addr, SocketAddr};
use std::process::ExitCode;

use phoneme_core::config::RestApiConfig;
use phoneme_core::Config;

use server::AppState;

/// Why the server cannot start, given a config. Returned by [`startup_plan`] so
/// the gating logic is unit-testable without binding a socket.
#[derive(Debug, PartialEq, Eq)]
pub enum StartupError {
    /// `[rest_api] enabled` is false — the bridge is opt-in.
    Disabled,
}

/// The loopback socket the server should bind, or why it must refuse to start.
///
/// Pure: takes the parsed `[rest_api]` config, returns either the bind address
/// (always on `127.0.0.1`) or a [`StartupError`]. `main` does the actual bind
/// and I/O; this function holds the policy (off-by-default guard + loopback
/// pinning) so both are tested in isolation.
pub fn startup_plan(cfg: &RestApiConfig) -> Result<SocketAddr, StartupError> {
    if !cfg.enabled {
        return Err(StartupError::Disabled);
    }
    // Loopback only — see the module docs. The port is configurable; the
    // interface is not.
    Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, cfg.port)))
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = match Config::load_resolved() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to load config: {e}");
            return ExitCode::FAILURE;
        }
    };

    let addr = match startup_plan(&cfg.rest_api) {
        Ok(addr) => addr,
        Err(StartupError::Disabled) => {
            eprintln!(
                "error: the local REST API is disabled.\n\
                 hint: enable it by setting `[rest_api] enabled = true` in your config \
                 (it binds 127.0.0.1 only)."
            );
            return ExitCode::FAILURE;
        }
    };

    let state = AppState {
        pipe_name: cfg.daemon.pipe_name.clone(),
    };
    let app = server::router(state);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: failed to bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(%addr, "phoneme-rest listening (loopback only)");
    eprintln!(
        "phoneme-rest listening on http://{addr} (loopback only) — forwarding to daemon pipe '{}'",
        cfg.daemon.pipe_name
    );

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("error: server stopped: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_config_refuses_to_start() {
        let cfg = RestApiConfig {
            enabled: false,
            port: 3737,
        };
        assert_eq!(startup_plan(&cfg), Err(StartupError::Disabled));
    }

    #[test]
    fn default_config_is_disabled_on_port_3737() {
        let cfg = RestApiConfig::default();
        assert!(!cfg.enabled, "rest_api must be off by default");
        assert_eq!(cfg.port, 3737, "default port must be 3737");
        // The default config therefore refuses to start.
        assert_eq!(startup_plan(&cfg), Err(StartupError::Disabled));
    }

    #[test]
    fn enabled_config_binds_loopback_only() {
        let cfg = RestApiConfig {
            enabled: true,
            port: 3737,
        };
        let addr = startup_plan(&cfg).expect("enabled config should yield a bind addr");
        assert!(
            addr.ip().is_loopback(),
            "must bind loopback only, got {addr}"
        );
        assert_eq!(addr.port(), 3737);
        // Belt-and-suspenders: it is exactly 127.0.0.1, never 0.0.0.0.
        assert_eq!(addr.ip(), std::net::Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn enabled_config_honors_custom_port() {
        let cfg = RestApiConfig {
            enabled: true,
            port: 9999,
        };
        let addr = startup_plan(&cfg).unwrap();
        assert_eq!(addr.port(), 9999);
        assert!(addr.ip().is_loopback());
    }

    /// The bridge is reachable from a top-level `Config`'s `[rest_api]` section
    /// and that section is off by default — so a freshly-defaulted Config never
    /// starts the server.
    #[test]
    fn config_default_carries_disabled_rest_api() {
        let cfg = Config::default();
        assert!(!cfg.rest_api.enabled);
        assert_eq!(cfg.rest_api.port, 3737);
    }
}
