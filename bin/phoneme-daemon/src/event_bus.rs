//! Daemon event bus — broadcast channel for `SubscribeEvents` consumers.
//!
//! The one-way half of the daemon's nervous system: every stage of a
//! recording's life (recorder start/stop, pipeline stage changes, LLM
//! activity, queue depth, tag changes) is `emit`ted here, and each
//! `SubscribeEvents` connection holds its own receiver that `ipc_handler`
//! forwards down the pipe.
//!
//! Delivery contract: fire-and-forget fan-out over a fixed 64-slot tokio
//! broadcast channel. Emitting never blocks and never fails — zero
//! subscribers is normal (a headless daemon). A subscriber that falls more
//! than the buffer behind is disconnected by its forwarding loop (it sees
//! `Lagged`) and is expected to reconnect and re-fetch state, which is why
//! no daemon code path may ever *depend* on an event being delivered.

use phoneme_ipc::DaemonEvent;
use tokio::sync::broadcast;

const BUS_CAPACITY: usize = 64;

/// Cloneable handle to the daemon-wide broadcast channel. All clones share
/// one sender; receivers are minted per subscription via [`EventBus::subscribe`].
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<DaemonEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Subscribe to events. Returns a new Receiver each call.
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.tx.subscribe()
    }

    /// Emit an event. Returns the receiver count for diagnostics; OK if zero.
    pub fn emit(&self, event: DaemonEvent) {
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoneme_core::RecordingId;

    #[tokio::test]
    async fn subscriber_receives_emitted_events() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let id = RecordingId::new();
        bus.emit(DaemonEvent::TranscriptionStarted { id: id.clone() });
        let received = rx.recv().await.unwrap();
        assert!(matches!(received, DaemonEvent::TranscriptionStarted { id: rid } if rid == id));
    }

    #[tokio::test]
    async fn emit_with_no_subscribers_does_not_panic() {
        let bus = EventBus::new();
        bus.emit(DaemonEvent::WhisperStatusChanged { reachable: false });
    }
}
