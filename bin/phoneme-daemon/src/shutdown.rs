//! Graceful shutdown coordinator — one watch channel every long-lived task
//! observes.
//!
//! The single [`ShutdownCoordinator`] lives in `AppState`, so the IPC
//! `Shutdown` handler, the Ctrl+C listener (`install_signals`), and `main`'s
//! failure paths all flip the one shared flag, which is what makes `phoneme
//! daemon stop` actually stop the daemon. Consumers (queue worker, whisper
//! supervisors, retention loop, the IPC serve select) hold a
//! [`ShutdownSignal`] (or a raw receiver) and wind down when it fires; `main`
//! then finalizes any in-flight recording and awaits them. The trigger is
//! sticky and idempotent: late subscribers see the flag already set, and a
//! second trigger is harmless.

use tokio::sync::watch;

#[derive(Clone)]
pub struct ShutdownSignal {
    rx: watch::Receiver<bool>,
}

impl ShutdownSignal {
    /// `true` if shutdown has been signaled.
    pub fn is_shutting_down(&self) -> bool {
        *self.rx.borrow()
    }

    /// Wait until shutdown is signaled.
    pub async fn wait(&mut self) {
        while !*self.rx.borrow() {
            if self.rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// Clone the inner `watch::Receiver<bool>` for tasks that consume it
    /// directly (e.g. `queue_worker::run`).
    pub fn clone_receiver(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }
}

pub struct ShutdownCoordinator {
    tx: watch::Sender<bool>,
    pub signal: ShutdownSignal,
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl ShutdownCoordinator {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            tx,
            signal: ShutdownSignal { rx },
        }
    }

    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }

    /// Install Ctrl+C handler. Returns immediately after starting the
    /// background listener.
    pub fn install_signals(&self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("Ctrl+C received");
                let _ = tx.send(true);
            }
        });
    }
}
