//! `phoneme suggest-tasks <ID>` — run the LLM task-extraction step on a recording
//! on demand (the CLI face of the GUI ✅ Extract button).
//!
//! Spawning path, beside `suggest-entities` / `suggest-tags`. Sends `SuggestTasks`,
//! which — like `SuggestEntities` — awaits the model: the Ok reply arrives after
//! the structured tasks land on the recording (`TasksUpdated` fires for open GUI
//! views; any `done` flag on a surviving task is preserved). Review the result with
//! `phoneme show <ID>` or `phoneme tasks`. Errors when the recording has no
//! transcript yet (`invalid_config`) or the id is unknown (`not_found`).

use crate::args::SuggestTasksArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: SuggestTasksArgs, cfg: &Config, json: bool) -> ExitCode {
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

    match client.send(Request::SuggestTasks { id }).await {
        Ok(_) => {
            if !json {
                println!("tasks extracted (review with `phoneme tasks` or `phoneme show <id>`)");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    #[tokio::test]
    async fn sends_suggest_tasks() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("suggest-tasks", |_req| {
            Response::Ok(serde_json::Value::Null)
        });
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(SuggestTasksArgs { id: id.to_string() }, &cfg, false),
        )
        .await
        .expect("suggest-tasks must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(mock.received(), vec![Request::SuggestTasks { id }]);
    }
}
