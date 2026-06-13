//! `phoneme speaker rename|clear <ID> <LABEL> [NAME]` — name a recording's
//! diarized speaker labels (the CLI face of the GUI speaker chips).
//!
//! Spawning path. `rename` sends `SetSpeakerName { id, speaker_label, name }`
//! with the user-chosen name; `clear` sends the same request with a blank
//! `name`, which the daemon treats as "drop the mapping" (the label reverts to
//! "Speaker N"). The `LABEL` is the 1-based `[Speaker N]` index from the
//! transcript — the daemon errors on a label below 1. The stored transcript
//! keeps its canonical `[Speaker N]` markers, so a rename is reversible and
//! never rewrites the text.

use crate::args::{SpeakerAction, SpeakerArgs};
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: SpeakerArgs, cfg: &Config, json: bool) -> ExitCode {
    let (id_str, label, name, renaming) = match args.action {
        SpeakerAction::Rename { id, label, name } => (id, label, name, true),
        // A blank name is the daemon's "clear this mapping" signal.
        SpeakerAction::Clear { id, label } => (id, label, String::new(), false),
    };

    let id = match RecordingId::parse(id_str.as_str()) {
        Some(id) => id,
        None => {
            eprintln!("error: '{id_str}' is not a valid recording id");
            return ExitCode::FAILURE;
        }
    };
    if label < 1 {
        eprintln!(
            "error: speaker label must be 1 or greater (it is the 1-based [Speaker N] index)"
        );
        return ExitCode::FAILURE;
    }

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    match client
        .send(Request::SetSpeakerName {
            id,
            speaker_label: label,
            name,
        })
        .await
    {
        Ok(_) => {
            if !json {
                if renaming {
                    println!("speaker {label} renamed");
                } else {
                    println!("speaker {label} name cleared");
                }
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

    async fn run_speaker(action: SpeakerAction) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("speaker", |_req| Response::Ok(serde_json::Value::Null));
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(SpeakerArgs { action }, &cfg, false),
        )
        .await
        .expect("speaker must return promptly");
        (code, mock.received())
    }

    #[tokio::test]
    async fn rename_sends_set_speaker_name() {
        let id = RecordingId::new();
        let (code, reqs) = run_speaker(SpeakerAction::Rename {
            id: id.to_string(),
            label: 2,
            name: "Sarah".into(),
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::SetSpeakerName {
                id,
                speaker_label: 2,
                name: "Sarah".into()
            }]
        );
    }

    #[tokio::test]
    async fn clear_sends_blank_name() {
        let id = RecordingId::new();
        let (code, reqs) = run_speaker(SpeakerAction::Clear {
            id: id.to_string(),
            label: 1,
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::SetSpeakerName {
                id,
                speaker_label: 1,
                name: String::new()
            }],
            "clear sends an empty name, the daemon's drop-mapping signal"
        );
    }

    #[tokio::test]
    async fn label_below_one_is_rejected_before_any_request() {
        // A bad label must fail locally without auto-spawning a daemon.
        let cfg = Config::default();
        let code = run(
            SpeakerArgs {
                action: SpeakerAction::Rename {
                    id: RecordingId::new().to_string(),
                    label: 0,
                    name: "x".into(),
                },
            },
            &cfg,
            false,
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }
}
