//! Daemon-forwarding client used by the REST handlers.
//!
//! ## Connect strategy: one short-lived pipe connection per request
//!
//! Each REST call opens a fresh [`NamedPipeTransport`] to the daemon, sends its
//! one [`Request`], reads the one [`Response`], and drops the connection. No
//! pool, no shared long-lived connection. This is the simplest correct choice
//! for a thin bridge:
//!
//! - The IPC protocol is strictly one-response-per-request on a connection, and
//!   responses are ordered. A pooled/shared connection would have to serialize
//!   concurrent HTTP requests behind a mutex anyway, erasing the benefit; a
//!   fresh connection per request lets the daemon's per-connection accept loop
//!   handle them concurrently instead.
//! - Pipe connect is cheap (local named pipe, ~sub-millisecond) and the daemon
//!   re-arms its listener immediately after each accept, so connection churn is
//!   not a bottleneck for a localhost admin/automation surface.
//! - It mirrors the CLI, which also connects per invocation
//!   (`bin/phoneme/src/client.rs`) — we reuse that exact connect path.
//!
//! Unlike the CLI's spawning `Client::connect`, this bridge **never
//! auto-spawns** the daemon: an HTTP client asking the bridge to reach a daemon
//! that isn't running should get a clean `503`, not have a daemon silently
//! started on its behalf (the observe-only posture, matching the CLI's
//! `connect_observe`).

use std::time::Duration;

use phoneme_ipc::{
    IpcTransportError, NamedPipeTransport, Request, Response, Transport, TransportResult,
};

use crate::error::RestError;

/// Upper bound on a single daemon round-trip. Connecting is already bounded by
/// the transport's busy-retry deadline; this bounds the *response* read so a
/// wedged daemon (pipe accepted but no reply) can't park an HTTP task and its
/// live pipe handle forever behind this persistent server. The CLI reuses
/// `request()` as a short-lived process, so a hang is self-limiting there — this
/// server is long-running, so it needs an explicit bound.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Forward one [`Request`] to the daemon and return the decoded JSON value on
/// success, mapping a [`Response::Err`] to a [`RestError::Daemon`] and any
/// transport failure — including a response that never arrives within
/// [`REQUEST_TIMEOUT`] — to [`RestError::Transport`] (→ 503).
///
/// `pipe_name` is the configured `daemon.pipe_name`; the connection is opened
/// and dropped within this call (see the module docs for why per-request).
pub async fn forward(pipe_name: &str, req: Request) -> Result<serde_json::Value, RestError> {
    let mut transport = NamedPipeTransport::connect(pipe_name).await?;
    // Bound the response read: a wedged daemon must surface as 503, never hang.
    let resp = tokio::time::timeout(REQUEST_TIMEOUT, transport.request(req))
        .await
        .map_err(|_elapsed| {
            IpcTransportError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "daemon did not respond within the request timeout",
            ))
        })??;
    match resp {
        Response::Ok(value) => Ok(value),
        Response::Err(e) => Err(RestError::Daemon(e)),
    }
}

/// Open a fresh subscription connection to the daemon's event stream.
///
/// Returns the same boxed `Stream` of `DaemonEvent`s the CLI's `watch` command
/// consumes — the SSE handler adapts it into `text/event-stream` frames. A
/// separate connection from [`forward`] because the IPC protocol turns a
/// connection one-way the moment `SubscribeEvents` is sent.
pub async fn subscribe(
    pipe_name: &str,
) -> TransportResult<futures::stream::BoxStream<'static, TransportResult<phoneme_ipc::DaemonEvent>>>
{
    let mut transport = NamedPipeTransport::connect(pipe_name).await?;
    transport.subscribe().await
}
