//! `phoneme cleanup <ID>` — re-run only the LLM cleanup step on a stored
//! transcript (no re-transcription, so hand edits to other recordings and
//! the preserved original stay intact).
//!
//! Spawning path. Sends `RerunCleanup` with the one-run overrides from
//! flags (`--provider` — also forces the step on for this run, `--model`,
//! `--prompt`, `--api-url`, `--api-key`; none persisted). The daemon ACKs
//! immediately and cleans in the background; the result lands as a
//! `TranscriptUpdated` event, so check `phoneme show` (or `watch`) for the
//! new text. Exits 6 (invalid config) when post-processing isn't enabled.

use crate::args::CleanupArgs;
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: CleanupArgs, cfg: &Config, json: bool) -> ExitCode {
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
    match client
        .send(Request::RerunCleanup {
            id,
            model: args.model,
            provider: args.provider,
            prompt: args.prompt,
            api_url: args.api_url,
            api_key: args.api_key,
        })
        .await
    {
        Ok(value) => {
            if json {
                output::print_json(&value);
            } else if let Some(text) = value.as_str() {
                println!("{text}");
            } else {
                println!("cleanup complete");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
