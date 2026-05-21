use crate::client::Client;
use crate::exit;
use futures::StreamExt;
use phoneme_core::Config;
use std::process::ExitCode;

pub async fn run(cfg: &Config) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
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
