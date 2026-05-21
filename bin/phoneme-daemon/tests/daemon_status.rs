//! End-to-end: client connects, daemon responds to DaemonStatus.

mod common;

use common::DaemonHarness;
use phoneme_ipc::{Request, Response, Transport};

#[tokio::test]
async fn daemon_status_returns_running_and_pid() {
    let mut h = DaemonHarness::start().await;
    let resp = h.client.request(Request::DaemonStatus).await.unwrap();
    match resp {
        Response::Ok(value) => {
            assert_eq!(value["running"], true);
            assert!(value["pid"].is_number(), "pid should be a number");
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }
}

#[tokio::test]
async fn record_status_is_false_on_fresh_daemon() {
    let mut h = DaemonHarness::start().await;
    let resp = h.client.request(Request::RecordStatus).await.unwrap();
    match resp {
        Response::Ok(value) => {
            assert_eq!(value["recording"], false);
            assert!(value["id"].is_null());
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }
}
