//! phoneme-ipc — IPC schema and transport for Phoneme.

pub mod codec;
pub mod schema;

pub use codec::JsonLineCodec;
pub use schema::{DaemonEvent, IpcError, IpcErrorKind, Request, Response};
