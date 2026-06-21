//! `phoneme meeting …` — Meeting Mode (v1.6) from the terminal.
//!
//! A meeting records the microphone and the system audio (WASAPI loopback)
//! as two separate recordings linked by a shared `meeting_id`; both are
//! transcribed independently through the normal pipeline.
//!
//! Each subcommand maps 1:1 to an IPC request. The mutating verbs ride the
//! spawning path: `start` → `StartMeeting`, `stop` → `StopMeeting`, `toggle`
//! → `MeetingToggle` (atomic, for hotkey-style bindings), and `rename
//! <meeting_id> <name>` (or `--clear`) → `UpdateMeetingName`. `tracks
//! <meeting_id>` → `ListMeeting` (rendered like `phoneme list`) is read-only,
//! so it uses the observe-only path like `list`. The start/stop/toggle
//! replies print the `meeting_id` so scripts can chain into `tracks`.

use crate::args::{MeetingAction, MeetingArgs};
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, Recording};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: MeetingArgs, cfg: &Config, json: bool) -> ExitCode {
    // `tracks` returns a list of recordings, so it gets its own rendering path
    // (table / JSON-lines) rather than the meeting_id-printing path below. It is
    // a read-only inspection (like `list`), so it uses the observe-only path:
    // a down daemon is the answer, not something to fix by spawning one.
    if let MeetingAction::Tracks { meeting_id } = &args.action {
        let mut client = match Client::connect_observe(cfg).await {
            Ok(c) => c,
            Err(code) => return code,
        };
        let value = match client
            .send(Request::ListMeeting {
                meeting_id: meeting_id.clone(),
            })
            .await
        {
            Ok(v) => v,
            Err(code) => return code,
        };
        let rows: Vec<Recording> = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: parsing meeting tracks response: {e}");
                return ExitCode::FAILURE;
            }
        };
        if json {
            output::print_json_lines(&rows);
        } else {
            output::print_list_pretty(&rows);
        }
        return ExitCode::SUCCESS;
    }

    // Start/stop/toggle/rename mutate daemon state, so they ride the spawning
    // path: a missing daemon is started rather than treated as the answer.
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let req = match args.action {
        MeetingAction::Start => Request::StartMeeting,
        MeetingAction::Stop => Request::StopMeeting,
        MeetingAction::Toggle => Request::MeetingToggle,
        MeetingAction::Tracks { .. } => unreachable!("handled above"),
        // `--clear` (no NAME) sends `None`, which the daemon stores as a NULL
        // meeting_name (auto-generated label); a NAME sends `Some(name)`. An
        // empty positional NAME is rejected by clap's required/conflicts rules.
        MeetingAction::Rename {
            meeting_id,
            name,
            clear: _,
        } => Request::UpdateMeetingName { meeting_id, name },
        // The whole-meeting digest re-run mirrors `phoneme summarize`: the daemon
        // ACKs immediately and generates in the background, storing the result and
        // emitting `MeetingDigestUpdated` / `MeetingDigestFailed`.
        MeetingAction::Digest { meeting_id, model } => {
            Request::RerunMeetingDigest { meeting_id, model }
        }
    };

    // The digest re-run ACKs with `null` (it runs in the background), so it has no
    // `meeting_id` to echo — print a confirmation instead, mirroring `summarize`.
    let is_digest = matches!(req, Request::RerunMeetingDigest { .. });

    match client.send(req).await {
        Ok(value) => {
            if json {
                crate::output::print_json(&value);
            } else if let Some(session) = value.get("meeting_id").and_then(|v| v.as_str()) {
                println!("{session}");
            } else if is_digest {
                println!("meeting digest requested");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
