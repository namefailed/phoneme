//! phoneme-ipc — IPC schema and transport for Phoneme.

pub mod codec;
pub mod error;
pub mod named_pipe;
pub mod schema;
pub mod transport;

pub use codec::JsonLineCodec;
pub use error::{IpcTransportError, TransportResult};
pub use named_pipe::{pipe_path, NamedPipeConnection, NamedPipeListener, NamedPipeTransport};
pub use schema::{
    DaemonEvent, IpcError, IpcErrorKind, PipelineStage, RerunAllOverrides, Request, Response,
    ServerRequest,
};
pub use transport::Transport;
