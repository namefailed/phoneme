//! `phoneme edit <ID> [--text … | metadata flags]` — hand-edit a recording's
//! transcript and/or metadata.
//!
//! Spawning path. Three independent edits, applied in one invocation when
//! combined:
//! - transcript: `--text` (or stdin) → `UpdateTranscript`. Pipe-friendly:
//!   `fix-typos < old.txt | phoneme edit <id>`.
//! - title: `--title "…"` → `SetRecordingTitle { title: Some(..) }` (marks the
//!   title user-owned); `--clear-title` → `SetRecordingTitle { title: None }`
//!   (reverts to auto-generation on the next pipeline run).
//! - favorite: `--favorite` / `--unfavorite` → `SetFavorite`.
//! - pinned: `--pin` / `--unpin` → `SetPinned`.
//!
//! Stdin is only consulted when a transcript edit is actually requested — a
//! metadata-only edit (e.g. just `--favorite` or `--pin`) never blocks reading
//! stdin.

use crate::args::EditArgs;
use crate::client::Client;
use crate::exit;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::io::Read;
use std::process::ExitCode;

pub async fn run(args: EditArgs, cfg: &Config) -> ExitCode {
    let id = match RecordingId::parse(args.id.as_str()) {
        Some(id) => id,
        None => {
            eprintln!("error: '{}' is not a valid recording id", args.id);
            return ExitCode::FAILURE;
        }
    };

    let title_edit = args.title.is_some() || args.clear_title;
    let favorite_edit = args.favorite || args.unfavorite;
    let pin_edit = args.pin || args.unpin;

    // A transcript edit happens when --text is given, or when there is no
    // metadata edit at all (the original stdin-driven behavior). A
    // metadata-only edit must not block reading stdin.
    let transcript_text = if args.text.is_some() {
        args.text
    } else if title_edit || favorite_edit || pin_edit {
        None
    } else {
        let mut buf = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
            eprintln!("error: reading transcript from stdin: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
        Some(buf)
    };

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    if let Some(text) = transcript_text {
        if let Err(code) = client
            .send(Request::UpdateTranscript {
                id: id.clone(),
                text,
            })
            .await
        {
            return code;
        }
        println!("transcript updated");
    }

    if title_edit {
        // --clear-title (and --title "") revert to auto; --title sets a
        // user-owned title.
        let title = args.title.filter(|t| !t.is_empty());
        if let Err(code) = client
            .send(Request::SetRecordingTitle {
                id: id.clone(),
                title: title.clone(),
            })
            .await
        {
            return code;
        }
        match title {
            Some(_) => println!("title updated"),
            None => println!("title cleared (will regenerate on the next run)"),
        }
    }

    if favorite_edit {
        let favorite = args.favorite;
        if let Err(code) = client
            .send(Request::SetFavorite {
                id: id.clone(),
                favorite,
            })
            .await
        {
            return code;
        }
        println!("{}", if favorite { "favorited" } else { "unfavorited" });
    }

    if pin_edit {
        let pinned = args.pin;
        if let Err(code) = client
            .send(Request::SetPinned {
                id: id.clone(),
                pinned,
            })
            .await
        {
            return code;
        }
        println!("{}", if pinned { "pinned" } else { "unpinned" });
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    fn base_args(id: &str) -> EditArgs {
        EditArgs {
            id: id.to_string(),
            text: None,
            title: None,
            clear_title: false,
            favorite: false,
            unfavorite: false,
            pin: false,
            unpin: false,
        }
    }

    async fn run_edit(args: EditArgs) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("edit", |_req| Response::Ok(serde_json::Value::Null));
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code = tokio::time::timeout(Duration::from_secs(5), run(args, &cfg))
            .await
            .expect("edit must return promptly");
        (code, mock.received())
    }

    #[tokio::test]
    async fn title_sets_a_user_owned_title() {
        let id = RecordingId::new();
        let mut args = base_args(&id.to_string());
        args.title = Some("Weekly sync".into());
        let (code, reqs) = run_edit(args).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::SetRecordingTitle {
                id,
                title: Some("Weekly sync".into())
            }]
        );
    }

    #[tokio::test]
    async fn clear_title_reverts_to_auto() {
        let id = RecordingId::new();
        let mut args = base_args(&id.to_string());
        args.clear_title = true;
        let (code, reqs) = run_edit(args).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::SetRecordingTitle { id, title: None }]);
    }

    #[tokio::test]
    async fn empty_title_string_clears_to_auto() {
        let id = RecordingId::new();
        let mut args = base_args(&id.to_string());
        args.title = Some(String::new());
        let (_code, reqs) = run_edit(args).await;
        assert_eq!(
            reqs,
            vec![Request::SetRecordingTitle { id, title: None }],
            "--title \"\" must clear back to auto, not set an empty title"
        );
    }

    #[tokio::test]
    async fn favorite_and_unfavorite_send_set_favorite() {
        let id = RecordingId::new();
        let mut args = base_args(&id.to_string());
        args.favorite = true;
        let (code, reqs) = run_edit(args).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::SetFavorite {
                id: id.clone(),
                favorite: true
            }]
        );

        let mut args = base_args(&id.to_string());
        args.unfavorite = true;
        let (_code, reqs) = run_edit(args).await;
        assert_eq!(
            reqs,
            vec![Request::SetFavorite {
                id,
                favorite: false
            }]
        );
    }

    #[tokio::test]
    async fn pin_and_unpin_send_set_pinned() {
        let id = RecordingId::new();
        let mut args = base_args(&id.to_string());
        args.pin = true;
        let (code, reqs) = run_edit(args).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::SetPinned {
                id: id.clone(),
                pinned: true
            }]
        );

        let mut args = base_args(&id.to_string());
        args.unpin = true;
        let (_code, reqs) = run_edit(args).await;
        assert_eq!(reqs, vec![Request::SetPinned { id, pinned: false }]);
    }

    #[tokio::test]
    async fn text_and_title_apply_both_edits() {
        let id = RecordingId::new();
        let mut args = base_args(&id.to_string());
        args.text = Some("Corrected.".into());
        args.title = Some("My title".into());
        let (code, reqs) = run_edit(args).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![
                Request::UpdateTranscript {
                    id: id.clone(),
                    text: "Corrected.".into()
                },
                Request::SetRecordingTitle {
                    id,
                    title: Some("My title".into())
                },
            ]
        );
    }
}
