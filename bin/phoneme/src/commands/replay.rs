use crate::args::IdArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: IdArgs, cfg: &Config) -> ExitCode {
    let id = RecordingId::from_string(args.id);
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.send(Request::ReplayRecording { id }).await {
        Ok(_) => {
            println!("replay queued");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
