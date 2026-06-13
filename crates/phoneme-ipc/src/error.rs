//! Transport-layer errors — connection and framing failures.
//!
//! These describe problems *reaching or speaking to* the daemon (connect
//! refused, pipe closed, malformed frame). They never cross the wire; a
//! request that reached the daemon and failed there comes back as a normal
//! [`crate::schema::IpcError`] inside `Response::Err` instead.

use thiserror::Error;

/// Errors that arise from the IPC transport layer (vs. the daemon's own
/// response errors, which are `IpcError`).
#[derive(Debug, Error)]
pub enum IpcTransportError {
    /// Dialing (or binding) the pipe failed at the OS level.
    #[error("connection failed: {0}")]
    Connect(#[source] std::io::Error),

    /// Another phoneme-daemon instance already owns the pipe name (bind-time
    /// `ERROR_ACCESS_DENIED`/`ERROR_PIPE_BUSY`).
    #[error("pipe is already owned by another phoneme-daemon")]
    AlreadyInUse,

    /// The peer closed the connection (also returned for a `request()` on a
    /// transport already consumed by `subscribe()`).
    #[error("connection closed by peer")]
    Closed,

    /// An I/O error mid-stream — including codec failures (a non-JSON or
    /// over-size frame surfaces as `InvalidData` here).
    #[error("transport I/O: {0}")]
    Io(#[from] std::io::Error),

    /// A frame parsed as JSON but not as the expected message type.
    #[error("malformed response: {0}")]
    Malformed(String),

    /// A local serialization/bookkeeping failure that is neither I/O nor the
    /// peer's fault.
    #[error("internal: {0}")]
    Internal(String),
}

/// Shorthand result for transport operations.
pub type TransportResult<T> = std::result::Result<T, IpcTransportError>;
