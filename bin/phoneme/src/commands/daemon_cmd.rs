use crate::args::{DaemonAction, DaemonArgs};
use crate::auto_spawn;
use crate::client::Client;
use crate::exit;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: DaemonArgs, cfg: &Config, json: bool) -> ExitCode {
    match args.action.unwrap_or(DaemonAction::Status) {
        DaemonAction::Start => match auto_spawn::ensure_running(cfg).await {
            Ok(()) => {
                println!("daemon started");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(exit::GENERIC_FAIL)
            }
        },
        DaemonAction::Stop => {
            let mut client = match Client::connect(cfg).await {
                Ok(c) => c,
                Err(code) => return code,
            };
            match client.send(Request::Shutdown).await {
                Ok(_) => {
                    println!("shutdown requested");
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
        DaemonAction::Status => {
            let mut client = match Client::connect(cfg).await {
                Ok(c) => c,
                Err(code) => return code,
            };
            match client.send(Request::DaemonStatus).await {
                Ok(value) => {
                    if json {
                        crate::output::print_json(&value);
                    } else {
                        println!("running: {}", value["running"]);
                        println!("pid:     {}", value["pid"]);
                    }
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
    }
}
