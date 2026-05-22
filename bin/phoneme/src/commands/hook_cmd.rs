use crate::args::{HookAction, HookArgs};
use crate::client::Client;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: HookArgs, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match args.action {
        HookAction::Test => match client
            .send(Request::HookTest {
                custom_command: None,
            })
            .await
        {
            Ok(value) => {
                if json {
                    crate::output::print_json(&value);
                } else {
                    println!("hook test:");
                    println!("  exit_code:   {}", value["exit_code"]);
                    println!("  duration_ms: {}", value["duration_ms"]);
                    if let Some(stderr) = value.get("stderr_tail").and_then(|v| v.as_str()) {
                        if !stderr.is_empty() {
                            println!("  stderr:      {stderr}");
                        }
                    }
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
    }
}
