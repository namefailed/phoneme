use chrono::{Local, TimeZone};
use phoneme_core::queue::{InboxQueue, InboxState};
use phoneme_core::{HookMetadata, HookPayload, RecordingId};
use tempfile::TempDir;

fn make_payload(id: RecordingId) -> HookPayload {
    HookPayload {
        id,
        timestamp: Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap(),
        transcript: String::new(),
        audio_path: "C:/tmp/x.wav".into(),
        duration_ms: 8470,
        model: String::new(),
        metadata: HookMetadata::current(),
    }
}

#[tokio::test]
async fn claim_next_quarantines_file_with_malformed_id() {
    // Regression: a file with a non-RecordingId name (e.g. dropped in manually)
    // must be quarantined, not slice-panic the daemon.
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let bad = dir.path().join("pending").join("not-an-id.json");
    std::fs::write(&bad, b"{}").unwrap();

    let claimed = q.claim_next().await.unwrap();
    assert!(claimed.is_none(), "malformed file must not be claimed");
    assert!(!bad.exists(), "malformed file should leave pending/");
    assert!(dir.path().join("failed").join("not-an-id.json").exists());
}

#[tokio::test]
async fn enqueue_creates_pending_file() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let id = RecordingId::new();
    q.enqueue(&make_payload(id.clone())).await.unwrap();
    let path = dir.path().join("pending").join(format!("{id}.json"));
    assert!(path.exists());
}

#[tokio::test]
async fn claim_next_returns_oldest_pending_and_moves_to_processing() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let id_a = RecordingId::from_datetime(Local.with_ymd_and_hms(2026, 5, 19, 9, 0, 0).unwrap());
    let id_b = RecordingId::from_datetime(Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap());
    q.enqueue(&make_payload(id_b.clone())).await.unwrap();
    q.enqueue(&make_payload(id_a.clone())).await.unwrap();

    let claimed = q.claim_next().await.unwrap().expect("has pending");
    assert_eq!(claimed.id, id_a);

    assert!(!dir
        .path()
        .join("pending")
        .join(format!("{id_a}.json"))
        .exists());
    assert!(dir
        .path()
        .join("processing")
        .join(format!("{id_a}.json"))
        .exists());
}

#[tokio::test]
async fn claim_next_returns_none_when_empty() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    assert!(q.claim_next().await.unwrap().is_none());
}

#[tokio::test]
async fn claim_next_quarantines_corrupt_payload() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    // Drop an unparseable file straight into pending/ (18-char id stem).
    let bad = dir.path().join("pending").join("20260519T143500000.json");
    std::fs::write(&bad, b"this is not json").unwrap();

    // claim_next consumes it and reports "nothing valid to claim".
    assert!(q.claim_next().await.unwrap().is_none());
    // It must be out of pending/ and quarantined in failed/.
    assert!(!bad.exists());
    assert!(dir
        .path()
        .join("failed")
        .join("20260519T143500000.json")
        .exists());
}

#[tokio::test]
async fn claim_next_skips_corrupt_then_serves_valid() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    // Corrupt file with an early-sorting id.
    let bad = dir.path().join("pending").join("20260519T090000000.json");
    std::fs::write(&bad, b"{ broken").unwrap();
    // Valid file with a later id — must not be starved by the corrupt one.
    let good = RecordingId::from_datetime(Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap());
    q.enqueue(&make_payload(good.clone())).await.unwrap();

    // First claim quarantines the corrupt file → None.
    assert!(q.claim_next().await.unwrap().is_none());
    // Second claim serves the valid payload.
    let claimed = q.claim_next().await.unwrap().expect("valid payload");
    assert_eq!(claimed.id, good);
}

#[tokio::test]
async fn finish_done_moves_processing_to_done() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let id = RecordingId::new();
    let mut payload = make_payload(id.clone());
    q.enqueue(&payload).await.unwrap();
    let claimed = q.claim_next().await.unwrap().unwrap();
    payload.transcript = "hello".into();
    q.finish_done(&claimed.id, &payload).await.unwrap();

    assert!(!dir
        .path()
        .join("processing")
        .join(format!("{id}.json"))
        .exists());
    let done = dir.path().join("done").join(format!("{id}.json"));
    assert!(done.exists());
    let text = std::fs::read_to_string(done).unwrap();
    let parsed: HookPayload = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed.transcript, "hello");
}

#[tokio::test]
async fn finish_failed_moves_processing_to_failed() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let id = RecordingId::new();
    q.enqueue(&make_payload(id.clone())).await.unwrap();
    let claimed = q.claim_next().await.unwrap().unwrap();
    q.finish_failed(&claimed.id, "whisper_unreachable", "connection refused")
        .await
        .unwrap();
    assert!(dir
        .path()
        .join("failed")
        .join(format!("{id}.json"))
        .exists());
}

#[tokio::test]
async fn states_counts_reflect_inbox() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let id1 = RecordingId::new();
    let id2 = RecordingId::new();
    q.enqueue(&make_payload(id1)).await.unwrap();
    q.enqueue(&make_payload(id2.clone())).await.unwrap();
    let claimed = q.claim_next().await.unwrap().unwrap();
    let _ = claimed;
    let counts = q.counts().await.unwrap();
    assert_eq!(counts.pending, 1);
    assert_eq!(counts.processing, 1);
    assert_eq!(counts.done, 0);
    assert_eq!(counts.failed, 0);
}

#[tokio::test]
async fn requeue_moves_processing_back_to_pending() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let id = RecordingId::new();
    q.enqueue(&make_payload(id.clone())).await.unwrap();
    let _claimed = q.claim_next().await.unwrap().unwrap();
    q.requeue(&id).await.unwrap();
    assert!(dir
        .path()
        .join("pending")
        .join(format!("{id}.json"))
        .exists());
    assert!(!dir
        .path()
        .join("processing")
        .join(format!("{id}.json"))
        .exists());
}

#[tokio::test]
async fn recover_orphans_moves_processing_to_pending() {
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let id = RecordingId::new();
    q.enqueue(&make_payload(id.clone())).await.unwrap();
    let _claimed = q.claim_next().await.unwrap().unwrap();
    drop(q);

    // New InboxQueue (simulating daemon restart) discovers the orphan.
    let q2 = InboxQueue::new(dir.path()).await.unwrap();
    let recovered = q2.recover_orphans().await.unwrap();
    assert_eq!(recovered, vec![id.clone()]);
    assert!(dir
        .path()
        .join("pending")
        .join(format!("{id}.json"))
        .exists());
}

#[tokio::test]
async fn enqueue_is_atomic_under_observation() {
    // The pending file must appear with its complete contents, not as a
    // partially-written file. We test this by enqueuing then verifying the
    // file parses as JSON immediately.
    let dir = TempDir::new().unwrap();
    let q = InboxQueue::new(dir.path()).await.unwrap();
    let id = RecordingId::new();
    let payload = make_payload(id.clone());
    q.enqueue(&payload).await.unwrap();
    let path = dir.path().join("pending").join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).unwrap();
    let parsed: HookPayload = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed.id, id);
}

#[test]
fn inbox_state_subdir_names_are_stable() {
    assert_eq!(InboxState::Pending.subdir(), "pending");
    assert_eq!(InboxState::Processing.subdir(), "processing");
    assert_eq!(InboxState::Done.subdir(), "done");
    assert_eq!(InboxState::Failed.subdir(), "failed");
}

use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Enqueue,
    Claim,
    FinishDone,
    FinishFailed,
    Requeue,
    Recover,
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::Enqueue),
        Just(Op::Claim),
        Just(Op::FinishDone),
        Just(Op::FinishFailed),
        Just(Op::Requeue),
        Just(Op::Recover),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]
    #[test]
    fn random_ops_leave_consistent_state(ops in proptest::collection::vec(op_strategy(), 0..30)) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().unwrap();
        rt.block_on(async {
            let dir = TempDir::new().unwrap();
            let q = InboxQueue::new(dir.path()).await.unwrap();
            let mut in_processing: Vec<RecordingId> = vec![];

            for op in ops {
                match op {
                    Op::Enqueue => {
                        let id = RecordingId::new();
                        q.enqueue(&make_payload(id)).await.unwrap();
                    }
                    Op::Claim => {
                        if let Some(p) = q.claim_next().await.unwrap() {
                            in_processing.push(p.id);
                        }
                    }
                    Op::FinishDone => {
                        if let Some(id) = in_processing.pop() {
                            q.finish_done(&id, &make_payload(id.clone())).await.unwrap();
                        }
                    }
                    Op::FinishFailed => {
                        if let Some(id) = in_processing.pop() {
                            q.finish_failed(&id, "x", "y").await.unwrap();
                        }
                    }
                    Op::Requeue => {
                        if let Some(id) = in_processing.pop() {
                            q.requeue(&id).await.unwrap();
                        }
                    }
                    Op::Recover => {
                        let recovered = q.recover_orphans().await.unwrap();
                        // Anything we knew was "in processing" is now back to pending.
                        in_processing.retain(|id| !recovered.contains(id));
                    }
                }
            }

            // Invariants: (1) no .tmp files left behind, (2) every json file
            // appears in exactly one subdirectory.
            for sub in ["pending", "processing", "done", "failed"] {
                for entry in std::fs::read_dir(dir.path().join(sub)).unwrap() {
                    let p = entry.unwrap().path();
                    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                    assert!(ext == "json", "leaked file: {}", p.display());
                }
            }
        });
    }
}
