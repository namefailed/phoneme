//! `phoneme summarize <ID>` — generate (or regenerate) a recording's LLM
//! summary on demand.
//!
//! Spawning path. Sends `RerunSummary` with the one-run `--model`/`--prompt`
//! overrides (never persisted). The daemon ACKs immediately and summarizes
//! in the background, storing the result and emitting `SummaryUpdated` /
//! `SummaryFailed` — `phoneme show` displays the stored summary. Exits 6
//! when no usable LLM provider is configured, 7 for an unknown id.

use crate::args::SummarizeArgs;
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: SummarizeArgs, cfg: &Config, json: bool) -> ExitCode {
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
        .send(Request::RerunSummary {
            id,
            model: args.model,
            prompt: args.prompt,
        })
        .await
    {
        Ok(value) => {
            if json {
                output::print_json(&value);
            } else if let Some(text) = value.as_str() {
                println!("{text}");
            } else {
                println!("summary generated");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
