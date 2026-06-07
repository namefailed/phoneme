//! Daemon event bus — broadcast channel for `SubscribeEvents` consumers.

use phoneme_ipc::DaemonEvent;
use tokio::sync::broadcast;

const BUS_CAPACITY: usize = 64;

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
