//! `phoneme edit <ID> [--text …]` — replace a recording's transcript with
//! hand-edited text (read from `--text`, else stdin — pipe-friendly:
//! `fix-typos < old.txt | phoneme edit <id>`).
//!
//! Spawning path. Sends `UpdateTranscript`; the daemon preserves the
//! original/unedited copies, re-embeds the new text for semantic search,
//! and emits `TranscriptUpdated`.

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

    // Take the new transcript from --text, or read it from stdin.
    let text = match args.text {
        Some(t) => t,
        None => {
            let mut buf = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("error: reading transcript from stdin: {e}");
                return ExitCode::from(exit::GENERIC_FAIL);
            }
            buf
        }
    };

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.send(Request::UpdateTranscript { id, text }).await {
        Ok(_) => {
            println!("transcript updated");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
