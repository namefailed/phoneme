use crate::args::DeleteArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: DeleteArgs, cfg: &Config) -> ExitCode {
    let id = RecordingId::from_string(args.id);
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .send(Request::DeleteRecording {
            id,
            keep_audio: args.keep_audio,
        })
        .await
    {
        Ok(_) => {
            println!("deleted");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
