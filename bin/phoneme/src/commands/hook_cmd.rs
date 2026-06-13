//! `phoneme hook test` — run the first configured hook once with a sample
//! payload and report exit code, duration, and (secret-redacted) stderr.
//!
//! Spawning path. Sends `HookTest { custom_command: None }` — the daemon
//! builds a representative payload so `{transcript}`-style placeholders are
//! exercised exactly as a real run would. The GUI Hook Manager's "test"
//! button uses the same request (with the in-edit command).

use crate::args::{HookAction, HookArgs};
use crate::client::Client;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: HookArgs, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match args.action {
        HookAction::Test => match client
            .send(Request::HookTest {
                custom_command: None,
            })
            .await
        {
            Ok(value) => {
                if json {
                    crate::output::print_json(&value);
                } else {
                    println!("hook test:");
                    println!("  exit_code:   {}", value["exit_code"]);
                    println!("  duration_ms: {}", value["duration_ms"]);
                    if let Some(stderr) = value.get("stderr_tail").and_then(|v| v.as_str()) {
                        if !stderr.is_empty() {
                            println!("  stderr:      {stderr}");
                        }
                    }
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
    }
}
