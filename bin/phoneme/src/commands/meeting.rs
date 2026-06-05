//! `phoneme meeting start|stop` — Meeting Mode (v1.6).
//!
//! Each subcommand maps 1:1 to an IPC request: `StartMeeting` / `StopMeeting`.
//! A meeting records the microphone and the system audio (WASAPI loopback) as
//! two separate recordings linked by a shared `session_id`; both are
//! transcribed independently through the normal pipeline.

use crate::args::{MeetingAction, MeetingArgs};
use crate::client::Client;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: MeetingArgs, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let req = match args.action {
        MeetingAction::Start => Request::StartMeeting,
        MeetingAction::Stop => Request::StopMeeting,
    };

    match client.send(req).await {
        Ok(value) => {
            if json {
                crate::output::print_json(&value);
            } else if let Some(session) = value.get("session_id").and_then(|v| v.as_str()) {
                println!("{session}");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
