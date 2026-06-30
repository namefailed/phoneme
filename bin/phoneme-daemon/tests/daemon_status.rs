//! End-to-end: client connects, daemon responds to DaemonStatus.

mod common;

use common::DaemonHarness;
use phoneme_ipc::{Request, Response, Transport};

#[tokio::test]
async fn daemon_status_returns_running_and_pid() {
    let mut h = DaemonHarness::start().await;
    // The OS pid of the daemon process the harness spawned — what DaemonStatus
    // must report as its own `pid`, not the test process's or a stale/zero value.
    let child_pid = h.daemon.id().expect("spawned daemon has a pid") as u64;
    let resp = h.client.request(Request::DaemonStatus).await.unwrap();
    match resp {
        Response::Ok(value) => {
            assert_eq!(value["running"], true);
            // Cross-process check: the answering process IS the spawned daemon.
            assert_eq!(
                value["pid"].as_u64(),
                Some(child_pid),
                "DaemonStatus must report the live daemon's own process id"
            );
            // The reported version is this build's version (test + daemon share
            // the crate, so CARGO_PKG_VERSION matches).
            assert_eq!(
                value["version"].as_str(),
                Some(env!("CARGO_PKG_VERSION")),
                "DaemonStatus reports the daemon's app version"
            );
            // The harness leaves whisper at its default bundled port (5809); the
            // preferred port mirrors that configured value.
            assert_eq!(
                value["whisper_preferred_port"].as_u64(),
                Some(5809),
                "whisper_preferred_port mirrors the configured bundled_server_port"
            );
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
