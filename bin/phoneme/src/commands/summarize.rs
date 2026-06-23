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
    // Keep the id for the status message — `id` is moved into the request below.
    let id_str = id.as_str().to_string();
    match client
        .send(Request::RerunSummary {
            id,
            model: args.model,
            prompt: args.prompt,
            // The CLI uses the configured summary connection (no per-run override).
            provider: None,
            api_url: None,
            api_key: None,
        })
        .await
    {
        Ok(_value) => {
            // The daemon ACKs immediately and summarizes in the background, so
            // the response carries no summary text — it lands later via
            // SummaryUpdated/SummaryFailed. Report that it was requested rather
            // than claiming it's already done.
            if json {
                output::print_json(&serde_json::json!({
                    "status": "requested",
                    "id": id_str,
                }));
            } else {
                println!(
                    "summary requested (generating in the background; view with `phoneme show {id_str}`)"
                );
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
