//! Connection to phoneme-daemon — the tray's single request/response pipe.
//!
//! [`Bridge`] wraps one `NamedPipeTransport` behind a mutex: every Tauri
//! command serializes through it (which is exactly why slow daemon work runs
//! detached on the daemon side — one stalled request would stall the whole
//! invoke surface). A failed request triggers one transparent
//! reconnect-and-retry, so an established bridge self-heals across daemon
//! restarts without the WebView noticing.
//!
//! [`BridgeSlot`] covers the other failure mode — never connected at all.
//! It is the lazily-reconnecting holder the rest of the tray actually talks
//! to: sync callers (hotkey handler, exit hook) `current()` a non-blocking
//! peek, async callers `get_or_connect()`, which re-runs auto-spawn +
//! connect under a write lock (concurrent callers reuse the winner's
//! connection) and caches the bridge for everyone. Event streaming does NOT
//! go through here — `events` opens its own dedicated subscription
//! connection, per the pipe protocol.

use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Mutex<NamedPipeTransport>>,
    pipe_name: String,
    pub config: Arc<Config>,
}

impl Bridge {
    pub async fn connect(config: Config) -> anyhow::Result<Self> {
        let pipe_name = config.daemon.pipe_name.clone();
        let transport = NamedPipeTransport::connect(&pipe_name).await?;
        Ok(Self {
            inner: Arc::new(Mutex::new(transport)),
            pipe_name,
            config: Arc::new(config),
        })
    }

    pub async fn reconnect(&self) -> anyhow::Result<()> {
        let new_transport = NamedPipeTransport::connect(&self.pipe_name).await?;
        let mut guard = self.inner.lock().await;
        *guard = new_transport;
        Ok(())
    }

    pub async fn request(&self, req: Request) -> anyhow::Result<Response> {
        let mut guard = self.inner.lock().await;
        match guard.request(req.clone()).await {
            Ok(r) => Ok(r),
            Err(_) => {
                drop(guard);
                self.reconnect().await?;
                let mut guard = self.inner.lock().await;
                Ok(guard.request(req).await?)
            }
        }
    }
}

/// Shared, lazily-reconnecting holder for the daemon [`Bridge`].
///
/// The tray can launch before the daemon accepts connections (cold boot,
/// crash-restart): startup's connect then fails, and before this slot existed
/// the managed `Option<Bridge>` stayed `None` for the tray's whole lifetime —
/// every command failed until an app restart, even though the startup log
/// promised "will retry on first action". The slot IS that retry: the first
/// caller that finds it empty re-runs the auto-spawn + connect and caches the
/// result for everyone. An ESTABLISHED bridge already self-heals per request
/// (see [`Bridge::request`]); the slot only covers the never-connected case.
#[derive(Clone)]
pub struct BridgeSlot {
    inner: Arc<RwLock<Option<Bridge>>>,
    /// False only in tests: a slot that never dials out, so unit tests can
    /// assert the disconnected error path without touching real pipes or
    /// spawning a real daemon.
    connect_enabled: bool,
}

impl BridgeSlot {
    pub fn new(initial: Option<Bridge>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(initial)),
            connect_enabled: true,
        }
    }

    /// A slot that never connects — for unit tests of the disconnected path.
    #[cfg(test)]
    pub fn offline() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            connect_enabled: false,
        }
    }

    /// Non-blocking peek for SYNC callers (the global-hotkey handler, the exit
    /// hook). `None` while disconnected — or while another task holds the
    /// write lock mid-connect, which those callers treat the same way.
    pub fn current(&self) -> Option<Bridge> {
        self.inner.try_read().ok().and_then(|g| g.clone())
    }

    /// The bridge, connecting first when the slot is empty (auto-spawning the
    /// daemon exactly like startup does). Concurrent callers serialize on the
    /// write lock; losers reuse the winner's connection instead of dialing
    /// their own.
    pub async fn get_or_connect(&self) -> Option<Bridge> {
        if let Some(b) = self.inner.read().await.clone() {
            return Some(b);
        }
        if !self.connect_enabled {
            return None;
        }
        let mut slot = self.inner.write().await;
        if let Some(b) = slot.clone() {
            return Some(b); // another caller connected while we waited
        }
        let config = crate::config_io::read().unwrap_or_default();
        if let Err(e) = crate::auto_spawn::ensure_running(&config).await {
            tracing::warn!(error = %e, "could not auto-spawn daemon on retry");
        }
        match Bridge::connect(config).await {
            Ok(b) => {
                tracing::info!("connected to daemon on retry");
                *slot = Some(b.clone());
                Some(b)
            }
            Err(e) => {
                tracing::warn!(error = %e, "daemon still unreachable");
                None
            }
        }
    }
}
