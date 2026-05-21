//! End-to-end: ListRecordings on a fresh daemon returns an empty array.

mod common;

use common::DaemonHarness;
use phoneme_core::ListFilter;
use phoneme_ipc::{Request, Response, Transport};

#[tokio::test]
async fn list_recordings_returns_empty_on_fresh_catalog() {
    let mut h = DaemonHarness::start().await;
    let resp = h
        .client
        .request(Request::ListRecordings {
            filter: ListFilter::default(),
        })
        .await
        .unwrap();
    match resp {
        Response::Ok(value) => {
            let arr = value
                .as_array()
                .expect("ListRecordings should return a JSON array");
            assert_eq!(arr.len(), 0, "fresh catalog should be empty");
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }
}

#[tokio::test]
async fn get_nonexistent_recording_returns_not_found() {
    let mut h = DaemonHarness::start().await;
    let bogus = phoneme_core::RecordingId::new();
    let resp = h
        .client
        .request(Request::GetRecording { id: bogus })
        .await
        .unwrap();
    match resp {
        Response::Err(e) => {
            assert_eq!(e.kind, phoneme_ipc::IpcErrorKind::NotFound);
        }
        Response::Ok(_) => panic!("expected not_found error"),
    }
}
