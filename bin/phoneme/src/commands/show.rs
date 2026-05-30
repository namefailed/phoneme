use crate::args::ShowArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Config, Recording, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: ShowArgs, cfg: &Config, json: bool) -> ExitCode {
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
    let value = match client.send(Request::GetRecording { id }).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let row: Recording = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: parsing show response: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };
    if args.audio_path_only {
        println!("{}", row.audio_path);
    } else if json {
        output::print_json(&serde_json::to_value(row).unwrap_or_default());
    } else {
        output::print_recording_pretty(&row);
    }
    ExitCode::SUCCESS
}
