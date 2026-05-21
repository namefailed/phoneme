//! phoneme-ipc — IPC schema and transport for Phoneme.

pub mod schema;

pub use schema::{DaemonEvent, IpcError, IpcErrorKind, Request, Response};
