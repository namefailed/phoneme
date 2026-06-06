#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue_worker::emit_queue_depth;
    use crate::app_state::AppState;
    use phoneme_core::types::HookPayload;
    use phoneme_core::Config;
    use phoneme_ipc::DaemonEvent;
    use std::time::Duration;

    async fn test_state(tmp: &std::path::Path) -> AppState {
        std::env::set_var("PHONEME_DATA_LOCAL", tmp.join("data"));
        let cfg = Config::default();
        AppState::new(cfg).await.expect("build test AppState")
    }

    #[tokio::test]
    async fn emit_queue_depth_sends_correct_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        let mut rx = state.events.subscribe();

        let payload = HookPayload {
            id: phoneme_core::id::RecordingId::new(),
            timestamp: chrono::Local::now(),
            transcript: "test".to_string(),
            audio_path: "test.wav".into(),
            duration_ms: 1000,
            model: "test".into(),
            metadata: phoneme_core::types::HookMetadata::current(),
        };
        state.inbox.enqueue(&payload).await.unwrap();

        // Fire the function we are testing
        emit_queue_depth(&state).await;

        // Drain existing events to find the QueueDepthChanged event
        let mut found = false;
        while let Ok(event) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            let event = event.unwrap();
            if let DaemonEvent::QueueDepthChanged { pending, processing, failed } = event {
                assert_eq!(pending, 1);
                assert_eq!(processing, 0);
                assert_eq!(failed, 0);
                found = true;
                break;
            }
        }
        assert!(found, "QueueDepthChanged event not emitted");
    }
}
