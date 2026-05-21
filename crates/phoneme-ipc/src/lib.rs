//! phoneme-ipc — IPC schema and transport for Phoneme.

pub mod codec;
pub mod error;
pub mod schema;
pub mod transport;

pub use codec::JsonLineCodec;
pub use error::{IpcTransportError, TransportResult};
pub use schema::{DaemonEvent, IpcError, IpcErrorKind, Request, Response};
pub use transport::Transport;
