//! `phoneme refire-hook <ID>` — re-run the post-processing hook against a
//! recording's already-stored transcript, without re-transcribing.
//!
//! Maps 1:1 to the `RefireHook` IPC request. The daemon runs the hook in the
//! background and reports the outcome via the `HookDone` / `HookFailed` events
//! (observe them with `phoneme watch`), so this command returns as soon as the
//! hook is queued. `--command` re-fires a specific hook instead of the
//! configured default; for safety the daemon only accepts a command already in
//! the configured hook allowlist (S-C2).

use crate::args::RefireHookArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: RefireHookArgs, cfg: &Config, json: bool) -> ExitCode {
    let id = match RecordingId::parse(args.id.as_str()) {
        Some(id) => id,
        None => {
            eprintln!("error: '{}' is not a valid recording id", args.id);
            return ExitCode::FAILURE;
        }
    };
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .send(Request::RefireHook {
            id,
            command: args.command,
        })
        .await
    {
        Ok(value) => {
            if json {
                crate::output::print_json(&value);
            } else {
                println!("hook re-fired (watch events for the result)");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
