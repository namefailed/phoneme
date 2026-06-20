//! Queue-worker unit tests: the depth-event emission contract — a real
//! `AppState` (temp-dir inbox/catalog) enqueues a payload and asserts the
//! exact `QueueDepthChanged` counts subscribers receive — plus the pure
//! transient-failure classifier that decides requeue-vs-give-up.

#[cfg(test)]
use crate::app_state::AppState;
use crate::queue_worker::{
    classify_transient_outcome, emit_queue_depth, TransientOutcome, MAX_TRANSIENT_ATTEMPTS,
};
use phoneme_core::types::HookPayload;
use phoneme_core::Config;
use phoneme_ipc::DaemonEvent;
use std::time::Duration;

async fn test_state(tmp: &std::path::Path) -> AppState {
    // Explicit data-local (no global `set_var`) so parallel tests don't race —
    // see `AppState::new_in`.
    let cfg = Config::default();
    AppState::new_in(cfg, Some(tmp.join("data")))
        .await
        .expect("build test AppState")
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
        if let DaemonEvent::QueueDepthChanged {
            pending,
            processing,
            failed,
        } = event
        {
            assert_eq!(pending, 1);
            assert_eq!(processing, 0);
            assert_eq!(failed, 0);
            found = true;
            break;
        }
    }
    assert!(found, "QueueDepthChanged event not emitted");
}

// ── transient-failure classifier ────────────────────────────────────────────

/// The first few consecutive transient misses requeue the item — a brief
/// whisper hiccup must not throw a recording into failed/.
#[test]
fn classifier_requeues_below_the_threshold() {
    for tries in 1..MAX_TRANSIENT_ATTEMPTS {
        assert_eq!(
            classify_transient_outcome(tries, MAX_TRANSIENT_ATTEMPTS),
            TransientOutcome::Requeue,
            "attempt {tries} of {MAX_TRANSIENT_ATTEMPTS} should still retry"
        );
    }
}

/// On the MAX_TRANSIENT_ATTEMPTS-th consecutive miss the item is given up, so a
/// permanently dead server can't loop one recording forever. The boundary is
/// inclusive: hitting the max exactly is already "give up".
#[test]
fn classifier_gives_up_at_the_threshold() {
    assert_eq!(
        classify_transient_outcome(MAX_TRANSIENT_ATTEMPTS, MAX_TRANSIENT_ATTEMPTS),
        TransientOutcome::GiveUp,
        "the max-th miss must give up, not retry again"
    );
    // And it stays GiveUp past the boundary (defensive — the worker removes the
    // counter on give-up, but the classifier must never flip back to Requeue).
    assert_eq!(
        classify_transient_outcome(MAX_TRANSIENT_ATTEMPTS + 3, MAX_TRANSIENT_ATTEMPTS),
        TransientOutcome::GiveUp
    );
}

/// The escalation walk a single persistently-failing recording takes: requeue
/// on every miss until the threshold, then give up exactly once. This is the
/// sequence the worker's per-recording `attempts` counter drives — modeled here
/// without the inbox/catalog I/O so the policy itself is pinned.
#[test]
fn classifier_escalates_requeue_then_give_up() {
    let mut tries = 0u32;
    let mut outcomes = Vec::new();
    // Simulate consecutive transient failures the way the worker does: bump the
    // counter, then classify.
    for _ in 0..MAX_TRANSIENT_ATTEMPTS {
        tries += 1;
        outcomes.push(classify_transient_outcome(tries, MAX_TRANSIENT_ATTEMPTS));
    }
    let requeues = outcomes
        .iter()
        .filter(|o| **o == TransientOutcome::Requeue)
        .count();
    let give_ups = outcomes
        .iter()
        .filter(|o| **o == TransientOutcome::GiveUp)
        .count();
    assert_eq!(
        requeues,
        (MAX_TRANSIENT_ATTEMPTS - 1) as usize,
        "every miss before the last requeues"
    );
    assert_eq!(give_ups, 1, "exactly the final miss gives up");
    assert_eq!(
        *outcomes.last().unwrap(),
        TransientOutcome::GiveUp,
        "give-up is the terminal step, not somewhere in the middle"
    );
}
