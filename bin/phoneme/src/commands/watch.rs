//! `phoneme watch` — tail the daemon's event stream as JSON lines.
//!
//! Observe-only: there is nothing to watch without a running daemon. Sends
//! `SubscribeEvents` and prints every `DaemonEvent` verbatim, one JSON
//! object per line, until the stream closes — the scripting counterpart of
//! the GUI's live updates (pipe through `jq` to filter). Exits 3 when the
//! daemon goes away mid-stream.

use crate::client::Client;
use crate::exit;
use futures::StreamExt;
use phoneme_core::Config;
use std::process::ExitCode;

pub async fn run(cfg: &Config) -> ExitCode {
    // Observe-only: there is nothing to watch without a running daemon.
    let mut client = match Client::connect_observe(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let mut events = match client.subscribe().await {
        Ok(s) => s,
        Err(code) => return code,
    };
    while let Some(item) = events.next().await {
        match item {
            Ok(event) => {
                if let Ok(s) = serde_json::to_string(&event) {
                    println!("{s}");
                }
            }
            Err(e) => {
                eprintln!("event stream ended: {e}");
                return ExitCode::from(exit::DAEMON_NOT_REACHABLE);
            }
        }
    }
    ExitCode::SUCCESS
}
