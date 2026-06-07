use crate::args::IdArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: IdArgs, cfg: &Config) -> ExitCode {
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
        .send(Request::RetranscribeRecording { id, model: None, run_hooks: None })
        .await
    {
        Ok(_) => {
            println!("re-transcribe queued");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
