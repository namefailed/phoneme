//! Shared test-only mock daemon, so command tests can assert the exact IPC
//! request a subcommand sends without standing up the real daemon.
//!
//! Each test gets a uniquely-named pipe (`MockDaemon::spawn`), points a
//! `Config` at it, runs the command's `run`, then inspects the captured
//! requests. The mock answers every request with a caller-supplied closure,
//! so a test can return the success value (or an error) a handler expects and
//! still see precisely what crossed the wire. Mirrors the inline mock in
//! `record.rs`, factored out for the parity-batch commands.

use phoneme_ipc::{NamedPipeListener, Request, Response, ServerRequest};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

/// A throwaway daemon stand-in bound to a unique pipe name. Records every
/// known request it receives and replies via the closure passed to `spawn`.
pub struct MockDaemon {
    /// The pipe name the client should dial — drop into `cfg.daemon.pipe_name`.
    pub pipe_name: String,
    /// Every known request the mock has answered, in arrival order.
    received: Arc<Mutex<Vec<Request>>>,
    handle: JoinHandle<()>,
}

impl MockDaemon {
    /// Bind a fresh pipe and start serving. `responder` maps each incoming
    /// request to the [`Response`] the mock should send back — return
    /// `Response::Ok(serde_json::Value::Null)` for the bare acknowledgement
    /// most mutations expect, or an `Err` to exercise the failure path.
    pub fn spawn<F>(label: &str, responder: F) -> Self
    where
        F: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        // A process-wide counter guarantees a unique pipe name even when two
        // tests spawn within the same timer tick under the parallel runner —
        // pid + nanos alone collided (the timer resolution is coarse on Windows),
        // which made `bind` below panic with "bind mock daemon pipe".
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let pid = std::process::id();
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pipe_name = format!("phoneme-cli-test-{label}-{pid}-{ns}-{seq}");

        let mut listener = NamedPipeListener::bind(&pipe_name).expect("bind mock daemon pipe");
        let received = Arc::new(Mutex::new(Vec::new()));
        let responder = Arc::new(responder);

        let handle = {
            let received = received.clone();
            tokio::spawn(async move {
                loop {
                    let Ok(mut conn) = listener.accept().await else {
                        break;
                    };
                    let received = received.clone();
                    let responder = responder.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(req)) = conn.recv().await {
                            let ServerRequest::Known(req) = req else {
                                continue;
                            };
                            let req = *req;
                            // The CLI client handshakes on connect (F3). Answer it
                            // transparently — compatible, not recorded, responder
                            // not invoked — so command tests still assert only the
                            // request their subcommand actually sends.
                            if let Request::Handshake { protocol_version } = req {
                                let resp = Response::Ok(serde_json::json!({
                                    "protocol_version": protocol_version,
                                    "app_version": env!("CARGO_PKG_VERSION"),
                                    "compatible": true,
                                }));
                                if conn.send_response(resp).await.is_err() {
                                    return;
                                }
                                continue;
                            }
                            // The spawning `Client::connect` path also probes
                            // `DaemonStatus` (via `auto_spawn::ensure_running`) to
                            // confirm the reachable daemon matches this build before
                            // reusing it. Answer transparently with a matching
                            // version — un-recorded, responder not invoked — so the
                            // mock reads as a healthy same-build daemon and command
                            // tests still assert only their subcommand's request.
                            if let Request::DaemonStatus = req {
                                let resp = Response::Ok(serde_json::json!({
                                    "running": true,
                                    "pid": std::process::id(),
                                    "version": env!("CARGO_PKG_VERSION"),
                                }));
                                if conn.send_response(resp).await.is_err() {
                                    return;
                                }
                                continue;
                            }
                            let response = responder(&req);
                            received.lock().unwrap().push(req);
                            if conn.send_response(response).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            })
        };

        Self {
            pipe_name,
            received,
            handle,
        }
    }

    /// Snapshot of every request answered so far, in arrival order.
    pub fn received(&self) -> Vec<Request> {
        self.received.lock().unwrap().clone()
    }
}

impl Drop for MockDaemon {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
