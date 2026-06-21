//! `phoneme tasks` — the cross-recording task list (the "everything I have to do"
//! view) plus a `done` sub-action to toggle one task.
//!
//! `phoneme tasks [--open]` is observe-only (`Client::connect_observe`): listing is
//! inspection, like `phoneme list` / `phoneme entities`, so "the daemon is down" is
//! a more useful answer than silently starting one. It sends `ListAllTasks` and
//! prints each task with its recording ref; `--open` shows only not-done tasks.
//!
//! `phoneme tasks done <RECORDING_ID> <TASK_ID>` (and `... undone`) toggles one
//! task's done flag via `SetTaskDone` on the spawning path (a mutation). The
//! `TASK_ID` is the row id shown by the list / `phoneme show`.

use crate::args::{TasksAction, TasksArgs};
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Config, RecordingId, TaskWithRecording};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: TasksArgs, cfg: &Config, json: bool) -> ExitCode {
    match args.action {
        Some(TasksAction::Done { id, task_id }) => set_done(&id, task_id, true, cfg, json).await,
        Some(TasksAction::Undone { id, task_id }) => set_done(&id, task_id, false, cfg, json).await,
        None => list(args.open, cfg, json).await,
    }
}

/// `phoneme tasks [--open]` — list every extracted task across the library.
async fn list(only_open: bool, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect_observe(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let value = match client.send(Request::ListAllTasks { only_open }).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let tasks: Vec<TaskWithRecording> = match serde_json::from_value(value) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: parsing task list: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };

    if json {
        output::print_json_lines(&tasks);
    } else if tasks.is_empty() {
        println!("no tasks yet (extract them with `phoneme suggest-tasks <id>`)");
    } else {
        for t in &tasks {
            // `[x]` done / `[ ]` open, then the text, an optional due hint, and the
            // recording ref so the user can act on it.
            let check = if t.done { "[x]" } else { "[ ]" };
            let due = t
                .due_hint
                .as_deref()
                .map(|d| format!(" (due: {d})"))
                .unwrap_or_default();
            let title = t.title.as_deref().unwrap_or(t.recording_id.as_str());
            println!(
                "{check} {}{due}  — {} [{} #{}]",
                t.text, title, t.recording_id, t.id
            );
        }
    }
    ExitCode::SUCCESS
}

/// `phoneme tasks done|undone <RECORDING_ID> <TASK_ID>` — toggle one task.
async fn set_done(id: &str, task_id: i64, done: bool, cfg: &Config, json: bool) -> ExitCode {
    let id = match RecordingId::parse(id) {
        Some(id) => id,
        None => {
            eprintln!("error: '{id}' is not a valid recording id");
            return ExitCode::FAILURE;
        }
    };

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    match client
        .send(Request::SetTaskDone { id, task_id, done })
        .await
    {
        Ok(_) => {
            if !json {
                println!(
                    "task #{task_id} marked {}",
                    if done { "done" } else { "not done" }
                );
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

    async fn run_tasks(
        args: TasksArgs,
        responder: impl Fn(&Request) -> Response + Send + Sync + 'static,
    ) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("tasks", responder);
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code = tokio::time::timeout(Duration::from_secs(5), run(args, &cfg, false))
            .await
            .expect("tasks must return promptly");
        (code, mock.received())
    }

    #[tokio::test]
    async fn sends_list_all_tasks() {
        let (code, reqs) = run_tasks(
            TasksArgs {
                open: false,
                action: None,
            },
            |_req| {
                Response::Ok(serde_json::json!([
                    { "recording_id": "r1", "title": "Standup", "id": 1, "text": "Send the report", "due_hint": "by Friday", "done": false },
                    { "recording_id": "r1", "title": "Standup", "id": 2, "text": "Book the room", "due_hint": null, "done": true },
                ]))
            },
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListAllTasks { only_open: false }]);
    }

    #[tokio::test]
    async fn open_flag_sets_only_open() {
        let (code, reqs) = run_tasks(
            TasksArgs {
                open: true,
                action: None,
            },
            |_req| Response::Ok(serde_json::json!([])),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListAllTasks { only_open: true }]);
    }

    #[tokio::test]
    async fn empty_list_still_succeeds() {
        let (code, reqs) = run_tasks(
            TasksArgs {
                open: false,
                action: None,
            },
            |_req| Response::Ok(serde_json::json!([])),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListAllTasks { only_open: false }]);
    }

    #[tokio::test]
    async fn done_subcommand_toggles_one_task() {
        let id = RecordingId::new();
        let id_str = id.to_string();
        let (code, reqs) = run_tasks(
            TasksArgs {
                open: false,
                action: Some(TasksAction::Done {
                    id: id_str,
                    task_id: 7,
                }),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::SetTaskDone {
                id,
                task_id: 7,
                done: true,
            }]
        );
    }
}
