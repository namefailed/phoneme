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
