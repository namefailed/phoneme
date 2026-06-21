//! `phoneme find-replace <ID> <FIND> <REPLACE> [--case-sensitive]` — literal
//! find-and-replace across a recording's stored transcript, or across **every**
//! recording with `--library`.
//!
//! Spawning path (it mutates). Per recording it sends one `FindReplace`; with
//! `--library` it sends one `FindReplaceLibrary` (positionals shift to FIND
//! REPLACE — no id). Either way the daemon does a **literal** (not regex)
//! substring replacement over the live transcript, case-insensitive by default
//! (`--case-sensitive` for exact case), preserving the original/clean baselines
//! and re-flowing the timing layers exactly like a hand edit. A zero-match
//! recording is left untouched. Prints how many occurrences were replaced (and,
//! for `--library`, how many recordings changed).

use crate::args::FindReplaceArgs;
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: FindReplaceArgs, cfg: &Config, json: bool) -> ExitCode {
    // Resolve the request from the positional slots, which shift when `--library`
    // drops the leading id (so `--library FIND REPLACE` lands FIND in the `id`
    // slot and REPLACE in the `find` slot).
    let request = if args.library {
        match (args.id, args.find, args.replace) {
            (Some(find), Some(replace), None) => Request::FindReplaceLibrary {
                find,
                replace,
                case_sensitive: args.case_sensitive,
            },
            (_, _, Some(_)) => {
                eprintln!("error: --library takes FIND and REPLACE only (no recording id)");
                return ExitCode::FAILURE;
            }
            _ => {
                eprintln!("error: usage: phoneme find-replace --library <FIND> <REPLACE>");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match (args.id, args.find, args.replace) {
            (Some(id), Some(find), Some(replace)) => {
                let id = match RecordingId::parse(id.as_str()) {
                    Some(id) => id,
                    None => {
                        eprintln!("error: '{id}' is not a valid recording id");
                        return ExitCode::FAILURE;
                    }
                };
                Request::FindReplace {
                    id,
                    find,
                    replace,
                    case_sensitive: args.case_sensitive,
                }
            }
            _ => {
                eprintln!(
                    "error: usage: phoneme find-replace <ID> <FIND> <REPLACE>  (or --library <FIND> <REPLACE>)"
                );
                return ExitCode::FAILURE;
            }
        }
    };

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let is_library = matches!(request, Request::FindReplaceLibrary { .. });
    let value = match client.send(request).await {
        Ok(v) => v,
        Err(code) => return code,
    };

    if json {
        output::print_json(&value);
        return ExitCode::SUCCESS;
    }

    if is_library {
        let recs = value
            .get("recordings_changed")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let total = value
            .get("total_replacements")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let failed = value.get("failed").and_then(|n| n.as_u64()).unwrap_or(0);
        match (recs, total) {
            (0, _) => println!("no matches across the library (nothing changed)"),
            (1, n) => println!("replaced {n} occurrence(s) in 1 recording"),
            (r, n) => println!("replaced {n} occurrence(s) across {r} recordings"),
        }
        // Some rows errored mid-sweep: say so plainly rather than letting the
        // smaller success count imply everything that could match was handled.
        if failed > 0 {
            eprintln!("warning: {failed} recording(s) failed to update (see the daemon log)");
        }
    } else {
        let replaced = value.get("replaced").and_then(|n| n.as_u64()).unwrap_or(0);
        match replaced {
            0 => println!("no matches (transcript unchanged)"),
            1 => println!("replaced 1 occurrence"),
            n => println!("replaced {n} occurrences"),
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    async fn run_fr(
        args: FindReplaceArgs,
        response: serde_json::Value,
    ) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("find-replace", move |_req| Response::Ok(response.clone()));
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
            id: Some(id.to_string()),
            find: Some("teh".into()),
            replace: Some("the".into()),
            library: false,
            case_sensitive: false,
        };
        let (code, reqs) = run_fr(args, serde_json::json!({ "replaced": 3 })).await;
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
            id: Some(id.to_string()),
            find: Some("API".into()),
            replace: Some("api".into()),
            library: false,
            case_sensitive: true,
        };
        let (_code, reqs) = run_fr(args, serde_json::json!({ "replaced": 1 })).await;
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
    async fn library_mode_shifts_positionals_and_sends_library_request() {
        // `--library FIND REPLACE`: FIND lands in the `id` slot, REPLACE in `find`.
        let args = FindReplaceArgs {
            id: Some("teh".into()),
            find: Some("the".into()),
            replace: None,
            library: true,
            case_sensitive: false,
        };
        let (code, reqs) = run_fr(
            args,
            serde_json::json!({ "recordings_changed": 2, "total_replacements": 5 }),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::FindReplaceLibrary {
                find: "teh".into(),
                replace: "the".into(),
                case_sensitive: false,
            }]
        );
    }

    #[tokio::test]
    async fn library_mode_rejects_a_third_positional() {
        // An id-looking third positional with --library is a usage error; the
        // command must fail before connecting (no daemon needed).
        let args = FindReplaceArgs {
            id: Some("a".into()),
            find: Some("b".into()),
            replace: Some("c".into()),
            library: true,
            case_sensitive: false,
        };
        let cfg = Config::default();
        let code = run(args, &cfg, false).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }

    #[tokio::test]
    async fn rejects_a_bad_recording_id() {
        let args = FindReplaceArgs {
            id: Some("not-an-id".into()),
            find: Some("x".into()),
            replace: Some("y".into()),
            library: false,
            case_sensitive: false,
        };
        // No daemon needed: the id is rejected before connecting.
        let cfg = Config::default();
        let code = run(args, &cfg, false).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }
}
