use crate::args::RetranscribeArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: RetranscribeArgs, cfg: &Config) -> ExitCode {
    let id = match RecordingId::parse(args.id.as_str()) {
        Some(id) => id,
        None => {
            eprintln!("error: '{}' is not a valid recording id", args.id);
            return ExitCode::FAILURE;
        }
    };

    // `--run-hooks` / `--no-run-hooks` map to Some(true)/Some(false); when
    // neither is given, `None` means "use the configured behavior".
    let run_hooks = if args.run_hooks {
        Some(true)
    } else if args.no_run_hooks {
        Some(false)
    } else {
        None
    };

    // `--no-post-process` is a one-time opt-out for this run only; otherwise
    // `None` uses the configured behavior.
    let post_process = if args.no_post_process {
        Some(false)
    } else {
        None
    };

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .send(Request::RetranscribeRecording {
            id,
            model: args.model,
            run_hooks,
            post_process,
            all_overrides: None,
        })
        .await
    {
        Ok(_) => {
            println!("re-transcribe queued");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
