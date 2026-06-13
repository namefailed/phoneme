//! Transport trait abstraction.
//!
//! Today's implementation is `NamedPipeTransport` (Windows named pipes); a
//! future v2.0 may add an `HttpTransport` for mobile clients without changing
//! the schema in `schema.rs`.

use crate::error::TransportResult;
use crate::schema::{DaemonEvent, Request, Response};
use async_trait::async_trait;
use futures::stream::BoxStream;

/// A client-side connection to the daemon: send requests, or convert the
/// connection into an event stream. Implemented by
/// [`crate::NamedPipeTransport`].
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send a single request and await its response.
    async fn request(&mut self, req: Request) -> TransportResult<Response>;

    /// Send `SubscribeEvents` and return a stream of events. The stream
    /// terminates when the connection closes.
    async fn subscribe(
        &mut self,
    ) -> TransportResult<BoxStream<'static, TransportResult<DaemonEvent>>>;
}
