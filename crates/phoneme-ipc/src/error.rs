use thiserror::Error;

/// Errors that arise from the IPC transport layer (vs. the daemon's own
/// response errors, which are `IpcError`).
#[derive(Debug, Error)]
pub enum IpcTransportError {
    #[error("connection failed: {0}")]
    Connect(#[source] std::io::Error),

    #[error("pipe is already owned by another phoneme-daemon")]
    AlreadyInUse,

    #[error("connection closed by peer")]
    Closed,

    #[error("transport I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("malformed response: {0}")]
    Malformed(String),

    #[error("internal: {0}")]
    Internal(String),
}

pub type TransportResult<T> = std::result::Result<T, IpcTransportError>;
