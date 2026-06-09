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

/// SDDL for the IPC pipe: a **protected** DACL (`P` — no inherited ACEs)
/// granting `GENERIC_ALL` to the object **owner** (the user who launched the
/// daemon) and to `LocalSystem`, and to no one else.
///
/// A named pipe created with the default security descriptor inherits the
/// token's default DACL, which on a normal interactive logon also grants every
/// other session `GENERIC_READ` — enough to read the daemon's event stream
/// (live transcripts) and, depending on configuration, to drive it. Pinning an
/// owner-only descriptor removes that exposure. (audit S-C1)
///
/// Note: `OW` (owner) resolves to the creating user for a normal, non-elevated
/// process — which is how Phoneme runs (a per-user app). It does not by itself
/// stop *same-user* code (that needs an auth token, tracked separately); it
/// closes the cross-user / cross-session hole.
const PIPE_SDDL: &str = "D:P(A;;GA;;;OW)(A;;GA;;;SY)";

/// Create a pipe-server instance whose kernel object carries [`PIPE_SDDL`]
/// instead of the permissive default descriptor.
///
/// Builds a `SECURITY_DESCRIPTOR` from the SDDL, points a `SECURITY_ATTRIBUTES`
/// at it, and hands that to tokio's raw constructor. The descriptor is
/// `LocalAlloc`'d by the conversion call and freed immediately after `create`
/// (which copies it into the new object).
fn create_secured_server(opts: &ServerOptions, path: &str) -> std::io::Result<NamedPipeServer> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

    let sddl_w: Vec<u16> = std::ffi::OsStr::new(PIPE_SDDL)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut psd: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `sddl_w` is a valid NUL-terminated UTF-16 string; on success the
    // call writes a freshly `LocalAlloc`'d descriptor into `psd`.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_w.as_ptr(),
            SDDL_REVISION_1,
            &mut psd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let mut sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: psd,
        bInheritHandle: 0,
    };

    // SAFETY: `sa` is a valid SECURITY_ATTRIBUTES whose descriptor stays live
    // until after this call; tokio passes it straight to CreateNamedPipeW, which
    // copies the descriptor into the kernel object.
    let result = unsafe {
        opts.create_with_security_attributes_raw(
            path,
            &mut sa as *mut SECURITY_ATTRIBUTES as *mut core::ffi::c_void,
        )
    };

    // SAFETY: `psd` was allocated by the conversion call above and is no longer
    // referenced after `create` returns.
    unsafe {
        LocalFree(psd as _);
    }

    result
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
        let mut opts = ServerOptions::new();
        opts.first_pipe_instance(true);
        let server = create_secured_server(&opts, &path).map_err(|e| {
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

        // Immediately create the next listener instance — same owner-only ACL.
        let next = create_secured_server(&ServerOptions::new(), &pipe_path(&self.name))
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
