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
use crate::commands::doctor::resolve_data_local_dir;
use crate::exit;
use phoneme_core::{voiceprint_eval, Catalog, Config, RecordingId};
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
    // `calibrate` is a local, read-only catalog analysis — it suggests a
    // threshold from your enrolled voices and never spawns a daemon or sends IPC,
    // so it's handled before the spawning request path below.
    if let SpeakerAction::Calibrate = args.action {
        return run_calibrate(cfg, json).await;
    }

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
        // Handled by the early return above (local, read-only, no IPC).
        SpeakerAction::Calibrate => unreachable!("calibrate is handled before the request path"),
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

/// Group enrolled voiceprints by their named-voice id into the labelled set the
/// EER harness consumes, preserving first-seen order so the output is stable.
/// `labeled` arrives ordered by voice id (see `Catalog::labeled_voiceprints`), so
/// same-voice rows are already contiguous, but we group defensively rather than
/// assume it.
fn group_by_voice(
    labeled: Vec<(String, Vec<f32>)>,
) -> Vec<(voiceprint_eval::SpeakerId, Vec<Vec<f32>>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<Vec<f32>>> =
        std::collections::HashMap::new();
    for (id, centroid) in labeled {
        match groups.get_mut(&id) {
            Some(v) => v.push(centroid),
            None => {
                order.push(id.clone());
                groups.insert(id, vec![centroid]);
            }
        }
    }
    order
        .into_iter()
        .map(|id| {
            let vecs = groups.remove(&id).unwrap_or_default();
            (id, vecs)
        })
        .collect()
}

/// `phoneme speaker calibrate` — suggest a `voiceprint_match_threshold` from the
/// enrolled voices. Read-only: opens the catalog the daemon owns (WAL allows a
/// concurrent reader, like `import-backup`'s open) and runs the pure EER metric
/// over the labelled captures. Never writes config — it prints a suggestion.
async fn run_calibrate(cfg: &Config, json: bool) -> ExitCode {
    let data_local = match resolve_data_local_dir() {
        Some(d) => d,
        None => {
            eprintln!("error: could not resolve data directory");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };
    let catalog_path = data_local.join("catalog.db");
    if !catalog_path.exists() {
        eprintln!(
            "error: no catalog at {} — nothing to calibrate yet",
            catalog_path.display()
        );
        return ExitCode::from(exit::NOT_FOUND);
    }
    let catalog = match Catalog::open(&catalog_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "error: could not open catalog at {}: {e}",
                catalog_path.display()
            );
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };

    let labeled = match catalog.labeled_voiceprints().await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: could not read voiceprints: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };
    let speakers = group_by_voice(labeled);
    let report = voiceprint_eval::calibrate(&speakers);

    let current = cfg.diarization.voiceprint_match_threshold;
    // Mean of a slice, or None when empty — for the human-readable summary.
    let mean = |xs: &[f32]| -> Option<f64> {
        if xs.is_empty() {
            None
        } else {
            Some(xs.iter().map(|&x| x as f64).sum::<f64>() / xs.len() as f64)
        }
    };
    let (genuine_scores, impostor_scores) = voiceprint_eval::trial_scores(&speakers);
    let named_voices = speakers.len();

    if json {
        let payload = serde_json::json!({
            "named_voices": named_voices,
            "genuine_trials": report.genuine_trials,
            "impostor_trials": report.impostor_trials,
            "intra_mean": mean(&genuine_scores),
            "inter_mean": mean(&impostor_scores),
            "eer": report.eer,
            "suggested_threshold": report.eer_threshold,
            "current_threshold": current,
        });
        println!("{payload}");
    }

    // The EER is only defined with at least one genuine AND one impostor trial,
    // i.e. two named voices, each with two or more captures. Below that, say so
    // plainly rather than print a meaningless number.
    if report.eer.is_none() || report.eer_threshold.is_none() {
        if !json {
            println!(
                "not enough labelled data to calibrate: {named_voices} named voice(s), \
                 {} same-voice pair(s), {} cross-voice pair(s).",
                report.genuine_trials, report.impostor_trials
            );
            println!(
                "enroll at least two named voices with two or more captures each, then re-run."
            );
            println!("current voiceprint_match_threshold = {current:.3} (unchanged).");
        }
        return ExitCode::SUCCESS;
    }

    if !json {
        let eer = report.eer.unwrap_or_default();
        let suggested = report.eer_threshold.unwrap_or_default();
        println!("speaker recognition calibration ({named_voices} named voices)");
        println!("  same-voice (genuine) pairs : {}", report.genuine_trials);
        println!("  cross-voice (impostor) pairs: {}", report.impostor_trials);
        if let Some(m) = mean(&genuine_scores) {
            println!("  intra (same-voice) mean cosine : {m:.3}");
        }
        if let Some(m) = mean(&impostor_scores) {
            println!("  inter (cross-voice) mean cosine: {m:.3}");
        }
        println!("  equal-error rate (EER): {:.1}%", eer * 100.0);
        println!("  suggested voiceprint_match_threshold: {suggested:.3}");
        println!("  current voiceprint_match_threshold:   {current:.3}");
        println!(
            "this only suggests — set it with: phoneme config set \
             diarization.voiceprint_match_threshold {suggested:.3}"
        );
    }
    ExitCode::SUCCESS
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

    #[test]
    fn group_by_voice_groups_and_preserves_order() {
        // Interleaved rows still collapse to one entry per voice, in first-seen
        // order — the labelled set the EER harness expects.
        let labeled = vec![
            ("ada".to_string(), vec![1.0, 0.0]),
            ("bob".to_string(), vec![0.0, 1.0]),
            ("ada".to_string(), vec![0.9, 0.1]),
            ("bob".to_string(), vec![0.1, 0.9]),
            ("ada".to_string(), vec![0.8, 0.2]),
        ];
        let grouped = group_by_voice(labeled);
        assert_eq!(grouped.len(), 2, "two distinct voices");
        assert_eq!(grouped[0].0, "ada", "first-seen voice first");
        assert_eq!(grouped[0].1.len(), 3, "all of ada's captures grouped");
        assert_eq!(grouped[1].0, "bob");
        assert_eq!(grouped[1].1.len(), 2);
    }

    #[test]
    fn group_by_voice_empty_is_empty() {
        assert!(group_by_voice(Vec::new()).is_empty());
    }
}
