//! `phoneme retranscribe <ID>` (alias `replay`) — queue a re-transcription.
//!
//! Spawning path: it creates queue work. Sends `RetranscribeRecording` with
//! the one-time overrides mapped from flags: `--model` (per-job model, never
//! persisted), `--run-hooks`/`--no-run-hooks` → `Some(true/false)` (absent =
//! configured behavior), `--no-post-process` → skip LLM cleanup this run, and
//! `--recipe ID|NAME` → the resolved `recipe_id` for the run (absent = the
//! default pipeline). Prints "re-transcribe queued" and exits — the run itself
//! happens when the queue worker claims the item (watch it via
//! `phoneme watch`/`queue`).

use crate::args::RetranscribeArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: RetranscribeArgs, cfg: &Config) -> ExitCode {
    let id = match RecordingId::parse(args.id.as_str()) {
        Some(id) => id,
        None => {
            eprintln!("error: '{}' is not a valid recording id", args.id);
            return ExitCode::FAILURE;
        }
    };

    // `--run-hooks` / `--no-run-hooks` map to Some(true)/Some(false); when
    // neither is given, `None` means "use the configured behavior".
    let run_hooks = if args.run_hooks {
        Some(true)
    } else if args.no_run_hooks {
        Some(false)
    } else {
        None
    };

    // `--no-post-process` is a one-time opt-out for this run only; otherwise
    // `None` uses the configured behavior.
    let post_process = if args.no_post_process {
        Some(false)
    } else {
        None
    };

    // `--recipe ID|NAME` is resolved locally to a stable recipe id (id first,
    // then case-insensitive name) against the same config the daemon reads;
    // absent = `None` = the default pipeline. An unmatched value errors here
    // with the available recipes rather than silently using the default.
    let recipe_id = match args.recipe.as_deref() {
        None => None,
        Some(v) => match crate::commands::recipe::resolve(cfg, v) {
            Ok(id) => Some(id),
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::FAILURE;
            }
        },
    };

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .send(Request::RetranscribeRecording {
            id,
            model: args.model,
            run_hooks,
            post_process,
            all_overrides: None,
            recipe_id,
        })
        .await
    {
        Ok(_) => {
            println!("re-transcribe queued");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::RetranscribeArgs;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    fn base_args(id: &str, recipe: Option<&str>) -> RetranscribeArgs {
        RetranscribeArgs {
            id: id.to_string(),
            model: None,
            run_hooks: false,
            no_run_hooks: false,
            no_post_process: false,
            recipe: recipe.map(str::to_string),
        }
    }

    /// `--recipe` by name resolves to the recipe's stable id on the wire.
    #[tokio::test]
    async fn recipe_by_name_wires_resolved_id() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("retx-recipe", |_req| Response::Ok(serde_json::Value::Null));
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(base_args(&id.to_string(), Some("Meeting notes")), &cfg),
        )
        .await
        .expect("retranscribe must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            mock.received(),
            vec![Request::RetranscribeRecording {
                id,
                model: None,
                run_hooks: None,
                post_process: None,
                all_overrides: None,
                recipe_id: Some("meeting_notes".into()),
            }]
        );
    }

    /// No `--recipe` keeps `recipe_id: None` (the default pipeline).
    #[tokio::test]
    async fn no_recipe_keeps_none() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("retx-none", |_req| Response::Ok(serde_json::Value::Null));
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(base_args(&id.to_string(), None), &cfg),
        )
        .await
        .expect("retranscribe must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            mock.received(),
            vec![Request::RetranscribeRecording {
                id,
                model: None,
                run_hooks: None,
                post_process: None,
                all_overrides: None,
                recipe_id: None,
            }]
        );
    }

    /// An unmatched `--recipe` errors before any request is sent.
    #[tokio::test]
    async fn unknown_recipe_errors_without_sending() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("retx-bad", |_req| Response::Ok(serde_json::Value::Null));
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(base_args(&id.to_string(), Some("no-such-recipe")), &cfg),
        )
        .await
        .expect("retranscribe must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
        assert!(
            mock.received().is_empty(),
            "a bad --recipe must not reach the daemon"
        );
    }
}
