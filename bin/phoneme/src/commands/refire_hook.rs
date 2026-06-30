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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    async fn run_refire(args: RefireHookArgs) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("refire-hook", |_req| {
            Response::Ok(serde_json::Value::Null)
        });
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code = tokio::time::timeout(Duration::from_secs(5), run(args, &cfg, false))
            .await
            .expect("refire-hook must return promptly");
        (code, mock.received())
    }

    /// No `--command`: re-fire the configured default hook — `RefireHook` with
    /// `command: None`, carrying the parsed recording id.
    #[tokio::test]
    async fn sends_refire_with_default_hook() {
        let id = RecordingId::new();
        let (code, reqs) = run_refire(RefireHookArgs {
            id: id.to_string(),
            command: None,
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::RefireHook { id, command: None }]);
    }

    /// `--command` forwards the chosen hook command verbatim.
    #[tokio::test]
    async fn sends_refire_with_explicit_command() {
        let id = RecordingId::new();
        let (code, reqs) = run_refire(RefireHookArgs {
            id: id.to_string(),
            command: Some("copy-to-clipboard".into()),
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::RefireHook {
                id,
                command: Some("copy-to-clipboard".into()),
            }]
        );
    }

    /// An invalid recording id is rejected locally (FAILURE) before connecting —
    /// no request reaches the daemon.
    #[tokio::test]
    async fn invalid_id_rejected_without_sending() {
        let (code, reqs) = run_refire(RefireHookArgs {
            id: "not-an-id".into(),
            command: None,
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
        assert!(reqs.is_empty(), "a bad id must not reach the daemon");
    }
}
