//! Smoke: the daemon answers a status request with a well-formed, meaningful
//! payload — the responding process is the daemon (real pid), it reports its
//! version, and the bundled whisper-server port wiring is present. Runs against
//! the test harness (a real spawned daemon on an isolated pipe) rather than a
//! hardcoded production pipe, so it is exercised in CI instead of `#[ignore]`d.

mod common;

use common::DaemonHarness;
use phoneme_ipc::{Request, Response, Transport};

#[tokio::test]
async fn smoke_daemon_status() {
    let mut h = DaemonHarness::start().await;
    let child_pid = h.daemon.id().expect("spawned daemon has a pid") as u64;

    let r = h.client.request(Request::DaemonStatus).await.unwrap();
    match r {
        Response::Ok(v) => {
            assert_eq!(v["running"], true);
            // The answering process is the spawned daemon, not a constant/zero.
            assert_eq!(
                v["pid"].as_u64(),
                Some(child_pid),
                "status pid must be the live daemon's process id"
            );
            // The version field carries this build's version.
            assert_eq!(
                v["version"].as_str(),
                Some(env!("CARGO_PKG_VERSION")),
                "status reports the daemon's app version"
            );
            // The preferred whisper port is present and mirrors the configured
            // default bundled port (5809) — clients dial this to reach the local
            // server.
            assert_eq!(
                v["whisper_preferred_port"].as_u64(),
                Some(5809),
                "status surfaces the configured bundled whisper port",
            );
        }
        _ => panic!("expected ok"),
    }
}
