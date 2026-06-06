use crate::args::{SessionAction, SessionArgs};
use crate::client;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: SessionArgs, cfg: &Config) -> ExitCode {
    match args.action {
        SessionAction::Rename { session_id, name } => {
            let mut client = match client::Client::connect(cfg).await {
                Ok(c) => c,
                Err(e) => return e,
            };
            let req = Request::UpdateSessionName {
                session_id,
                name: Some(name),
            };
            match client.send_silent(req).await {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => e,
            }
        }
    }
}
