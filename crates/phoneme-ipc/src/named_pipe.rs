//! Windows named-pipe transport for daemon ↔ client.
//!
//! - Server creates pipe instances at `\\.\pipe\<name>`, accepts one client
//!   per instance, immediately re-creates the listener.
//! - Client dials the same pipe name with `ClientOptions::open`.
//! - Both sides use `JsonLineCodec` for newline-delimited JSON.

use crate::codec::JsonLineCodec;
use crate::error::{IpcTransportError, TransportResult};
use crate::schema::{DaemonEvent, Request, Response};
use crate::transport::Transport;
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use tokio_util::codec::Framed;

/// The full Windows pipe name for a given short name.
pub fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\{name}")
}

/// Server-side: a single accepted connection. Use [`NamedPipeListener`] to
/// produce these.
pub struct NamedPipeConnection {
    framed_in: Framed<NamedPipeServer, JsonLineCodec<Request>>,
}

impl NamedPipeConnection {
    /// Receive the next request from the client, or `None` if they disconnected.
    pub async fn recv(&mut self) -> TransportResult<Option<Request>> {
        match self.framed_in.next().await {
            Some(Ok(req)) => Ok(Some(req)),
            Some(Err(e)) => Err(IpcTransportError::Io(e)),
            None => Ok(None),
        }
    }

    /// Send a Response back to the client.
    pub async fn send_response(&mut self, res: Response) -> TransportResult<()> {
        let json =
            serde_json::to_vec(&res).map_err(|e| IpcTransportError::Internal(e.to_string()))?;
        let io = self.framed_in.get_mut();
        use tokio::io::AsyncWriteExt;
        io.write_all(&json).await?;
        io.write_all(b"\n").await?;
        io.flush().await?;
        Ok(())
    }

    /// Send a DaemonEvent (for streaming subscriptions).
    pub async fn send_event(&mut self, event: DaemonEvent) -> TransportResult<()> {
        let json =
            serde_json::to_vec(&event).map_err(|e| IpcTransportError::Internal(e.to_string()))?;
        let io = self.framed_in.get_mut();
        use tokio::io::AsyncWriteExt;
        io.write_all(&json).await?;
        io.write_all(b"\n").await?;
        io.flush().await?;
        Ok(())
    }
}

/// Server-side listener. Each call to `accept` returns a connection and
/// immediately re-opens the listener for the next client.
pub struct NamedPipeListener {
    name: String,
    current: Option<NamedPipeServer>,
}

impl NamedPipeListener {
    /// Bind a fresh listener at the given pipe name. Returns an error if a
    /// previous server instance is still holding the name (per Windows
    /// semantics: first creator wins).
    pub fn bind(name: &str) -> TransportResult<Self> {
        let path = pipe_path(name);
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&path)
            .map_err(|e| {
                // ERROR_ACCESS_DENIED (5) or ERROR_PIPE_BUSY (231)
                // → another instance owns this pipe name.
                if matches!(e.raw_os_error(), Some(5) | Some(231)) {
                    IpcTransportError::AlreadyInUse
                } else {
                    IpcTransportError::Connect(e)
                }
            })?;
        Ok(Self {
            name: name.to_string(),
            current: Some(server),
        })
    }

    /// Accept the next incoming connection. Re-binds the listener for the
    /// following accept.
    pub async fn accept(&mut self) -> TransportResult<NamedPipeConnection> {
        let server = self
            .current
            .take()
            .ok_or_else(|| IpcTransportError::Internal("listener empty".into()))?;
        server.connect().await?;

        // Immediately create the next listener instance.
        let next = ServerOptions::new()
            .create(pipe_path(&self.name))
            .map_err(IpcTransportError::Connect)?;
        self.current = Some(next);

        let framed = Framed::new(server, JsonLineCodec::<Request>::new());
        Ok(NamedPipeConnection { framed_in: framed })
    }
}

/// Client-side connection.
///
/// `framed` is wrapped in `Option` so `subscribe()` can `take()` it without
/// needing a placeholder reconnect. After `subscribe()` succeeds, the
/// transport instance is consumed protocol-wise (further calls to `request`
/// will return `Closed`); the returned event stream owns the underlying IO.
pub struct NamedPipeTransport {
    framed: Option<Framed<NamedPipeClient, JsonLineCodec<Response>>>,
}

impl NamedPipeTransport {
    pub async fn connect(name: &str) -> TransportResult<Self> {
        let path = pipe_path(name);
        let client = loop {
            match ClientOptions::new().open(&path) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(231) => {
                    // ERROR_PIPE_BUSY — retry briefly
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => return Err(IpcTransportError::Connect(e)),
            }
        };
        Ok(Self {
            framed: Some(Framed::new(client, JsonLineCodec::<Response>::new())),
        })
    }
}

#[async_trait]
impl Transport for NamedPipeTransport {
    async fn request(&mut self, req: Request) -> TransportResult<Response> {
        let framed = self.framed.as_mut().ok_or(IpcTransportError::Closed)?;
        // Encode + send the request.
        let json =
            serde_json::to_vec(&req).map_err(|e| IpcTransportError::Internal(e.to_string()))?;
        let io = framed.get_mut();
        use tokio::io::AsyncWriteExt;
        io.write_all(&json).await?;
        io.write_all(b"\n").await?;
        io.flush().await?;

        // Await the response.
        match framed.next().await {
            Some(Ok(res)) => Ok(res),
            Some(Err(e)) => Err(IpcTransportError::Io(e)),
            None => Err(IpcTransportError::Closed),
        }
    }

    async fn subscribe(
        &mut self,
    ) -> TransportResult<BoxStream<'static, TransportResult<DaemonEvent>>> {
        let framed = self.framed.as_mut().ok_or(IpcTransportError::Closed)?;
        
        let json = serde_json::to_vec(&Request::SubscribeEvents)
            .map_err(|e| IpcTransportError::Internal(e.to_string()))?;
        let io = framed.get_mut();
        use tokio::io::AsyncWriteExt;
        io.write_all(&json).await?;
        io.write_all(b"\n").await?;
        io.flush().await?;

        // Take ownership of the framed IO and reframe with DaemonEvent codec.
        // After this point, self.framed is None — further request() calls return Closed.
        let old = self.framed.take().unwrap();
        let parts = old.into_parts();

        let mut new_parts =
            tokio_util::codec::FramedParts::new(parts.io, JsonLineCodec::<DaemonEvent>::new());
        new_parts.read_buf = parts.read_buf;
        new_parts.write_buf = parts.write_buf;

        let event_framed = Framed::from_parts(new_parts);
        let stream = event_framed.map(|r| r.map_err(IpcTransportError::Io));
        Ok(stream.boxed())
    }
}
