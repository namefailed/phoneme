//! phoneme-ipc — the wire contract between the Phoneme daemon and its clients.
//!
//! Everything the tray GUI, the `phoneme` CLI, and `phoneme-daemon` say to
//! each other is defined in this crate: the request/response/event types
//! ([`schema`]), the framing ([`codec`]), and the transport ([`named_pipe`],
//! abstracted by [`transport`] so a future HTTP transport can reuse the same
//! schema). The doc comments on [`schema`]'s types are the protocol
//! reference — each request documents its payload, the daemon's behavior, the
//! exact response JSON, and the events it triggers.
//!
//! ## Framing: NDJSON over a named pipe
//!
//! The daemon listens on a Windows named pipe (`\\.\pipe\<daemon.pipe_name>`,
//! default `phoneme`, owner-only ACL — see [`named_pipe`]). Every message in
//! either direction — request, response, or event — is exactly one
//! serde-JSON object followed by `\n` (newline-delimited JSON). There is no
//! length prefix and no batching; [`JsonLineCodec`] rejects any single
//! unterminated frame past 8 MiB instead of buffering it unbounded.
//!
//! ## Request/response, or subscribe → event stream
//!
//! A fresh connection is in request/response mode: the client writes one
//! [`Request`], the daemon answers with exactly one [`Response`]
//! (`{"status":"ok","value":…}` or `{"status":"err","value":{kind,message}}`),
//! and the pair repeats until the client hangs up. Requests on one connection
//! are answered strictly in order.
//!
//! [`Request::SubscribeEvents`] re-purposes the connection instead: the
//! daemon sends **no** acknowledging `Response` — from that line on, the
//! connection is a one-way stream of [`DaemonEvent`] JSON lines until either
//! side closes it. A client that wants events *and* commands therefore opens
//! two connections (the tray and the blocking CLI commands do exactly that).
//! Event fan-out is best-effort: a subscriber that falls behind the daemon's
//! broadcast buffer is disconnected and is expected to reconnect and re-fetch
//! state (`ListRecordings`, `QueueCounts`) rather than assume continuity.
//!
//! ## Compatibility rules
//!
//! The daemon, tray, and CLI are versioned together but can transiently skew
//! (rolling rebuilds, an old daemon still running). The schema therefore
//! evolves additively:
//!
//! - **New fields** on existing variants must carry `#[serde(default)]` so a
//!   line written by an older peer still decodes.
//! - **New variants** may be added freely. The daemon decodes incoming lines
//!   as [`ServerRequest`], so a request variant this daemon predates becomes
//!   [`ServerRequest::Unknown`] — answered with an error `Response` while the
//!   connection (and every other in-flight command on it) stays alive.
//! - **Removing or renaming** a variant or field is a breaking change to every
//!   surface at once; don't.
//! - Clients ignore JSON keys they don't recognize (serde's default) and must
//!   tolerate unknown event variants by skipping them.
//!
//! On top of additive evolution, [`PROTOCOL_VERSION`] gives the wire an explicit
//! integer version and [`Request::Handshake`] lets a client check it on connect:
//! the daemon reports its protocol + app version and whether the client's
//! protocol matches, so a hard-incompatible (post-breaking-change) client refuses
//! to operate with a clear message rather than misbehaving later. The handshake
//! is optional and best-effort — an unversioned old daemon, or any non-`false`
//! answer, lets the client proceed on the additive contract.

#![warn(missing_docs)]

pub mod codec;
pub mod error;
pub mod named_pipe;
pub mod schema;
pub mod transport;

/// Integer version of the IPC wire contract. Bumped only on a BREAKING schema
/// change (a removed/renamed variant or field — what additive evolution can't
/// cover); additive changes keep it stable. Carried in [`Request::Handshake`] so
/// a client built against a breaking revision can detect an incompatible daemon
/// up front and refuse cleanly. See the crate docs' "Protocol version" section.
pub const PROTOCOL_VERSION: u32 = 1;

pub use codec::JsonLineCodec;
pub use error::{IpcTransportError, TransportResult};
pub use named_pipe::{pipe_path, NamedPipeConnection, NamedPipeListener, NamedPipeTransport};
pub use schema::{
    DaemonEvent, IpcError, IpcErrorKind, PipelineStage, Request, RerunAllOverrides, Response,
    ServerRequest,
};
pub use transport::Transport;
