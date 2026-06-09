use crate::args::NotesArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Config, Recording, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: NotesArgs, cfg: &Config, json: bool) -> ExitCode {
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

    // With --set, update the notes; otherwise print the current notes.
    if let Some(notes) = args.set {
        match client.send(Request::UpdateNotes { id, notes }).await {
            Ok(_) => {
                if !json {
                    println!("notes updated");
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        }
    } else {
        let value = match client.send(Request::GetRecording { id }).await {
            Ok(v) => v,
            Err(code) => return code,
        };
        let row: Recording = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: parsing recording response: {e}");
                return ExitCode::from(exit::GENERIC_FAIL);
            }
        };
        let notes = row.notes.unwrap_or_default();
        if json {
            output::print_json(&serde_json::json!({ "notes": notes }));
        } else {
            println!("{notes}");
        }
        ExitCode::SUCCESS
    }
}
