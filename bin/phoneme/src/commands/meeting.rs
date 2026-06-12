//! `phoneme meeting start|stop` — Meeting Mode (v1.6).
//!
//! Each subcommand maps 1:1 to an IPC request: `StartMeeting` / `StopMeeting`.
//! A meeting records the microphone and the system audio (WASAPI loopback) as
//! two separate recordings linked by a shared `meeting_id`; both are
//! transcribed independently through the normal pipeline.

use crate::args::{MeetingAction, MeetingArgs};
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, Recording};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: MeetingArgs, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    // `tracks` returns a list of recordings, so it gets its own rendering path
    // (table / JSON-lines) rather than the meeting_id-printing path below.
    if let MeetingAction::Tracks { meeting_id } = &args.action {
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

    let req = match args.action {
        MeetingAction::Start => Request::StartMeeting,
        MeetingAction::Stop => Request::StopMeeting,
        MeetingAction::Toggle => Request::MeetingToggle,
        MeetingAction::Tracks { .. } => unreachable!("handled above"),
        MeetingAction::Rename { meeting_id, name } => Request::UpdateMeetingName {
            meeting_id,
            name: Some(name),
        },
    };

    match client.send(req).await {
        Ok(value) => {
            if json {
                crate::output::print_json(&value);
            } else if let Some(session) = value.get("meeting_id").and_then(|v| v.as_str()) {
                println!("{session}");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
