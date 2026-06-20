//! `phoneme find-replace <ID> <FIND> <REPLACE> [--case-sensitive]` — literal
//! find-and-replace across a recording's stored transcript.
//!
//! Spawning path (it mutates). Sends one `FindReplace` request: the daemon does
//! a **literal** (not regex) substring replacement over the live transcript,
//! case-insensitive by default (`--case-sensitive` for exact case), preserving
//! the original/clean baselines and re-flowing the timing layers exactly like a
//! hand edit. A zero-match is a no-op (nothing written). Prints the number of
//! occurrences replaced.

use crate::args::FindReplaceArgs;
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: FindReplaceArgs, cfg: &Config, json: bool) -> ExitCode {
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

    let value = match client
        .send(Request::FindReplace {
            id,
            find: args.find,
            replace: args.replace,
            case_sensitive: args.case_sensitive,
        })
        .await
    {
        Ok(v) => v,
        Err(code) => return code,
    };

    if json {
        output::print_json(&value);
        return ExitCode::SUCCESS;
    }

    let replaced = value.get("replaced").and_then(|n| n.as_u64()).unwrap_or(0);
    match replaced {
        0 => println!("no matches (transcript unchanged)"),
        1 => println!("replaced 1 occurrence"),
        n => println!("replaced {n} occurrences"),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    async fn run_fr(args: FindReplaceArgs, replaced: u64) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("find-replace", move |_req| {
            Response::Ok(serde_json::json!({ "replaced": replaced }))
        });
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code = tokio::time::timeout(Duration::from_secs(5), run(args, &cfg, false))
            .await
            .expect("find-replace must return promptly");
        (code, mock.received())
    }

    #[tokio::test]
    async fn sends_find_replace_request() {
        let id = RecordingId::new();
        let args = FindReplaceArgs {
            id: id.to_string(),
            find: "teh".into(),
            replace: "the".into(),
            case_sensitive: false,
        };
        let (code, reqs) = run_fr(args, 3).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::FindReplace {
                id,
                find: "teh".into(),
                replace: "the".into(),
                case_sensitive: false,
            }]
        );
    }

    #[tokio::test]
    async fn case_sensitive_flag_forwards() {
        let id = RecordingId::new();
        let args = FindReplaceArgs {
            id: id.to_string(),
            find: "API".into(),
            replace: "api".into(),
            case_sensitive: true,
        };
        let (_code, reqs) = run_fr(args, 1).await;
        assert_eq!(
            reqs,
            vec![Request::FindReplace {
                id,
                find: "API".into(),
                replace: "api".into(),
                case_sensitive: true,
            }]
        );
    }

    #[tokio::test]
    async fn rejects_a_bad_recording_id() {
        let args = FindReplaceArgs {
            id: "not-an-id".into(),
            find: "x".into(),
            replace: "y".into(),
            case_sensitive: false,
        };
        // No daemon needed: the id is rejected before connecting.
        let cfg = Config::default();
        let code = run(args, &cfg, false).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }
}
