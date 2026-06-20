//! `phoneme speaker rename|clear|reassign|merge|split …` — name and correct a
//! recording's diarized speaker labels (the CLI face of the GUI speaker chips
//! and the in-recording speaker correction, U1).
//!
//! Spawning path. `rename`/`clear` send `SetSpeakerName { id, speaker_label,
//! name }` (a blank `name` is the daemon's "drop the mapping" signal — the label
//! reverts to "Speaker N"); naming never rewrites the transcript text, so it's
//! reversible. `reassign`/`merge`/`split` (U1) actually change which segment
//! belongs to which speaker: the daemon keeps `transcript_segments` authoritative
//! and rebuilds the prose `[Speaker N]:` markers in the same transaction. Every
//! `LABEL` is the 1-based `[Speaker N]` index; `IDX` values are 0-based segment
//! indices (from `phoneme show --segments`). Bad labels are rejected locally
//! before any daemon is spawned.

use crate::args::{SpeakerAction, SpeakerArgs};
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

/// Parse + range-check a recording id and a speaker label, failing locally (no
/// daemon spawn) on bad input.
fn parse_id_and_label(id_str: &str, label: i64) -> Result<RecordingId, ExitCode> {
    let id = RecordingId::parse(id_str).ok_or_else(|| {
        eprintln!("error: '{id_str}' is not a valid recording id");
        ExitCode::FAILURE
    })?;
    if label < 1 {
        eprintln!(
            "error: speaker label must be 1 or greater (it is the 1-based [Speaker N] index)"
        );
        return Err(ExitCode::FAILURE);
    }
    Ok(id)
}

pub async fn run(args: SpeakerArgs, cfg: &Config, json: bool) -> ExitCode {
    // Build the request and the success line, validating ids/labels locally so a
    // bad arg never spawns a daemon.
    let (request, done) = match args.action {
        SpeakerAction::Rename { id, label, name } => {
            let id = match parse_id_and_label(&id, label) {
                Ok(id) => id,
                Err(code) => return code,
            };
            (
                Request::SetSpeakerName {
                    id,
                    speaker_label: label,
                    name,
                },
                format!("speaker {label} renamed"),
            )
        }
        SpeakerAction::Clear { id, label } => {
            let id = match parse_id_and_label(&id, label) {
                Ok(id) => id,
                Err(code) => return code,
            };
            (
                // A blank name is the daemon's "clear this mapping" signal.
                Request::SetSpeakerName {
                    id,
                    speaker_label: label,
                    name: String::new(),
                },
                format!("speaker {label} name cleared"),
            )
        }
        SpeakerAction::Reassign { id, idx, label } => {
            let id = match parse_id_and_label(&id, label) {
                Ok(id) => id,
                Err(code) => return code,
            };
            if idx < 0 {
                eprintln!("error: segment index must be 0 or greater");
                return ExitCode::FAILURE;
            }
            (
                Request::ReassignSegmentSpeaker {
                    id,
                    idx,
                    new_label: label,
                },
                format!("segment {idx} reassigned to speaker {label}"),
            )
        }
        SpeakerAction::Merge { id, from, into } => {
            let id = match parse_id_and_label(&id, from) {
                Ok(id) => id,
                Err(code) => return code,
            };
            if into < 1 {
                eprintln!("error: speaker label must be 1 or greater");
                return ExitCode::FAILURE;
            }
            if from == into {
                eprintln!("error: cannot merge a speaker into itself");
                return ExitCode::FAILURE;
            }
            (
                Request::MergeSpeakers {
                    id,
                    from_label: from,
                    into_label: into,
                },
                format!("speaker {from} merged into {into}"),
            )
        }
        SpeakerAction::Split {
            id,
            label,
            new_label,
            segments,
        } => {
            let id = match parse_id_and_label(&id, label) {
                Ok(id) => id,
                Err(code) => return code,
            };
            if new_label < 1 {
                eprintln!("error: speaker label must be 1 or greater");
                return ExitCode::FAILURE;
            }
            if label == new_label {
                eprintln!("error: split target label must differ from the source");
                return ExitCode::FAILURE;
            }
            if segments.iter().any(|&i| i < 0) {
                eprintln!("error: segment indices must be 0 or greater");
                return ExitCode::FAILURE;
            }
            let n = segments.len();
            (
                Request::SplitSpeaker {
                    id,
                    label,
                    segment_idxs: segments,
                    new_label,
                },
                format!("{n} segment(s) split from speaker {label} onto {new_label}"),
            )
        }
    };

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    match client.send(request).await {
        Ok(_) => {
            if !json {
                println!("{done}");
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

    #[tokio::test]
    async fn reassign_sends_reassign_segment_speaker() {
        let id = RecordingId::new();
        let (code, reqs) = run_speaker(SpeakerAction::Reassign {
            id: id.to_string(),
            idx: 3,
            label: 2,
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::ReassignSegmentSpeaker {
                id,
                idx: 3,
                new_label: 2,
            }]
        );
    }

    #[tokio::test]
    async fn merge_sends_merge_speakers() {
        let id = RecordingId::new();
        let (code, reqs) = run_speaker(SpeakerAction::Merge {
            id: id.to_string(),
            from: 2,
            into: 1,
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::MergeSpeakers {
                id,
                from_label: 2,
                into_label: 1,
            }]
        );
    }

    #[tokio::test]
    async fn split_sends_split_speaker_with_idx_list() {
        let id = RecordingId::new();
        let (code, reqs) = run_speaker(SpeakerAction::Split {
            id: id.to_string(),
            label: 1,
            new_label: 3,
            segments: vec![2, 4],
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::SplitSpeaker {
                id,
                label: 1,
                segment_idxs: vec![2, 4],
                new_label: 3,
            }]
        );
    }

    #[tokio::test]
    async fn merge_into_itself_is_rejected_before_any_request() {
        let cfg = Config::default();
        let code = run(
            SpeakerArgs {
                action: SpeakerAction::Merge {
                    id: RecordingId::new().to_string(),
                    from: 1,
                    into: 1,
                },
            },
            &cfg,
            false,
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }

    #[tokio::test]
    async fn split_onto_same_label_is_rejected_before_any_request() {
        let cfg = Config::default();
        let code = run(
            SpeakerArgs {
                action: SpeakerAction::Split {
                    id: RecordingId::new().to_string(),
                    label: 2,
                    new_label: 2,
                    segments: vec![0],
                },
            },
            &cfg,
            false,
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }
}
