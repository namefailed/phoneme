//! `phoneme dictation …` — the CLI face of the opt-in dictation re-grab history.
//!
//! Each subcommand maps 1:1 to a dictation-history IPC request (see
//! `phoneme-ipc`): history (`ListDictationHistory`), regrab (`RegrabDictation`),
//! forget (`DeleteDictationHistory`), clear (`ClearDictationHistory`).
//!
//! `history` is observe-only (`Client::connect_observe`) — listing is
//! inspection, like `phoneme queue list`, so "the daemon is down" is itself the
//! answer. The mutating verbs use the spawning path. `regrab` injects keystrokes
//! / pastes the stored text at the CURRENT cursor (the original window is long
//! gone), so it needs a live daemon to do the typing.

use crate::args::{DictationAction, DictationArgs};
use crate::client::Client;
use crate::output;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use phoneme_core::{Config, DictationHistoryEntry};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: DictationArgs, cfg: &Config, json: bool) -> ExitCode {
    // Only `history` is pure inspection; the rest mutate or inject text and need
    // an active daemon.
    let observe_only = matches!(args.action, DictationAction::History { .. });
    let mut client = if observe_only {
        match Client::connect_observe(cfg).await {
            Ok(c) => c,
            Err(code) => return code,
        }
    } else {
        match Client::connect(cfg).await {
            Ok(c) => c,
            Err(code) => return code,
        }
    };

    match args.action {
        DictationAction::History { limit } => {
            match client.send(Request::ListDictationHistory { limit }).await {
                Ok(value) => {
                    if json {
                        output::print_json(&value);
                    } else {
                        print_history_table(&value);
                    }
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
        DictationAction::Regrab {
            id,
            paste,
            type_mode,
        } => {
            // --paste / --type pick the delivery; neither flag → let the daemon
            // fall back to the configured `type_mode`.
            let mode = if paste {
                Some("paste".to_string())
            } else if type_mode {
                Some("type".to_string())
            } else {
                None
            };
            match client.send(Request::RegrabDictation { id, mode }).await {
                Ok(_) => {
                    if !json {
                        println!("re-inserted dictation {id} at the cursor");
                    }
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
        DictationAction::Forget { id } => {
            match client.send(Request::DeleteDictationHistory { id }).await {
                Ok(value) => {
                    if json {
                        output::print_json(&value);
                    } else if value
                        .get("removed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        println!("forgot dictation {id}");
                    } else {
                        println!("no dictation with id {id}");
                    }
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
        DictationAction::Clear => match client.send(Request::ClearDictationHistory).await {
            Ok(value) => {
                if json {
                    output::print_json(&value);
                } else {
                    let removed = value.get("removed").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("cleared {removed} dictation(s)");
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
    }
}

/// Render the dictation-history array (newest first) as a table.
fn print_history_table(value: &serde_json::Value) {
    let entries: Vec<DictationHistoryEntry> = match serde_json::from_value(value.clone()) {
        Ok(e) => e,
        Err(_) => {
            // Malformed shape — fall back to raw JSON rather than crash.
            output::print_json(value);
            return;
        }
    };
    if entries.is_empty() {
        println!("no dictation history (turn on [in_place].keep_history to record some)");
        return;
    }
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["id", "when", "app", "chars", "text"]);
    for e in &entries {
        // A short single-line preview of the text so a long dictation doesn't
        // blow up the table; the full text is re-grabbed, not read, from here.
        let preview = preview_text(&e.text, 60);
        table.add_row(vec![
            e.id.to_string(),
            e.created_at.clone(),
            e.app.clone().unwrap_or_default(),
            e.char_count.to_string(),
            preview,
        ]);
    }
    println!("{table}");
}

/// One-line, length-capped preview of a dictation: newlines flattened to spaces,
/// truncated on a char boundary with an ellipsis so multi-byte text is never cut
/// mid-character.
fn preview_text(text: &str, max_chars: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() <= max_chars {
        return flat.to_string();
    }
    let truncated: String = flat.chars().take(max_chars).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::DictationAction;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    async fn run_dictation(
        action: DictationAction,
        responder: impl Fn(&Request) -> Response + Send + Sync + 'static,
    ) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("dictation", responder);
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(DictationArgs { action }, &cfg, false),
        )
        .await
        .expect("dictation must return promptly");
        (code, mock.received())
    }

    #[tokio::test]
    async fn history_sends_list_request() {
        let (code, reqs) = run_dictation(DictationAction::History { limit: 50 }, |_req| {
            Response::Ok(serde_json::json!([
                { "id": 1, "text": "hello", "char_count": 5, "app": "code", "created_at": "2026-06-21T15:00:00Z" },
            ]))
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListDictationHistory { limit: 50 }]);
    }

    #[tokio::test]
    async fn empty_history_still_succeeds() {
        let (code, reqs) = run_dictation(DictationAction::History { limit: 50 }, |_req| {
            Response::Ok(serde_json::json!([]))
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListDictationHistory { limit: 50 }]);
    }

    #[tokio::test]
    async fn regrab_paste_flag_sends_paste_mode() {
        let (code, reqs) = run_dictation(
            DictationAction::Regrab {
                id: 7,
                paste: true,
                type_mode: false,
            },
            |_req| Response::Ok(serde_json::json!({})),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::RegrabDictation {
                id: 7,
                mode: Some("paste".into()),
            }]
        );
    }

    #[tokio::test]
    async fn regrab_no_flag_sends_none_mode() {
        let (_code, reqs) = run_dictation(
            DictationAction::Regrab {
                id: 7,
                paste: false,
                type_mode: false,
            },
            |_req| Response::Ok(serde_json::json!({})),
        )
        .await;
        assert_eq!(reqs, vec![Request::RegrabDictation { id: 7, mode: None }]);
    }

    #[tokio::test]
    async fn forget_and_clear_send_their_requests() {
        let (_c1, r1) = run_dictation(DictationAction::Forget { id: 3 }, |_req| {
            Response::Ok(serde_json::json!({ "removed": true }))
        })
        .await;
        assert_eq!(r1, vec![Request::DeleteDictationHistory { id: 3 }]);

        let (_c2, r2) = run_dictation(DictationAction::Clear, |_req| {
            Response::Ok(serde_json::json!({ "removed": 2 }))
        })
        .await;
        assert_eq!(r2, vec![Request::ClearDictationHistory]);
    }

    #[test]
    fn preview_text_flattens_and_truncates_on_char_boundary() {
        assert_eq!(preview_text("hi\nthere", 60), "hi there");
        // Multi-byte chars: never cut mid-character; ellipsis appended.
        let s = preview_text(&"é".repeat(10), 4);
        assert_eq!(s, "éééé…");
    }
}
