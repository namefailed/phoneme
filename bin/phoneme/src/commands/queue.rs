//! `phoneme queue …` — inspect and manage the transcription pipeline queue.
//!
//! Each subcommand maps 1:1 to a queue IPC request (see `phoneme-ipc`):
//! list (`ListQueue`), counts (`QueueCounts`), pause/resume (`SetQueuePaused`),
//! status (`QueuePaused`), reorder (`ReorderQueue`), cancel (`CancelQueued`),
//! cancel-processing (`CancelProcessing`), cancel-all (`CancelAllQueued`), and
//! clear-failed (`ClearFailed`). With no subcommand, defaults to `list` so a
//! bare `phoneme queue` shows what's in flight — mirroring the GUI queue panel.

use crate::args::{QueueAction, QueueArgs};
use crate::client::Client;
use crate::output;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: QueueArgs, cfg: &Config, json: bool) -> ExitCode {
    // Read-only inspection actions (list, counts, status) use the observe-only
    // path — if the daemon is down that is itself the answer. Mutating actions
    // (pause, resume, cancel, reorder, clear-failed) use the spawning path
    // because they require an active daemon to make the change.
    let observe_only = matches!(
        args.action,
        None | Some(QueueAction::List) | Some(QueueAction::Counts) | Some(QueueAction::Status)
    );
    let mut client = if observe_only {
        match Client::connect_observe(cfg).await {
            Ok(c) => c,
            Err(code) => return code,
        }
    } else {
        match Client::connect(cfg).await {
            Ok(c) => c,
            Err(code) => return code,
        }
    };

    match args.action.unwrap_or(QueueAction::List) {
        QueueAction::List => match client.send(Request::ListQueue).await {
            Ok(value) => {
                if json {
                    output::print_json(&value);
                } else {
                    print_queue_table(&value);
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        QueueAction::Counts => match client.send(Request::QueueCounts).await {
            Ok(value) => {
                if json {
                    output::print_json(&value);
                } else {
                    let n = |k: &str| value.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("pending:    {}", n("pending"));
                    println!("processing: {}", n("processing"));
                    println!("done:       {}", n("done"));
                    println!("failed:     {}", n("failed"));
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        QueueAction::Pause => set_paused(&mut client, true, json).await,
        QueueAction::Resume => set_paused(&mut client, false, json).await,
        QueueAction::Status => match client.send(Request::QueuePaused).await {
            Ok(value) => {
                if json {
                    output::print_json(&value);
                } else {
                    let paused = value
                        .get("paused")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    println!("{}", if paused { "paused" } else { "running" });
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        QueueAction::Reorder { ids } => {
            let parsed = match parse_ids(&ids) {
                Ok(p) => p,
                Err(code) => return code,
            };
            match client.send(Request::ReorderQueue { ids: parsed }).await {
                Ok(_) => {
                    if !json {
                        println!("queue reordered");
                    }
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
        QueueAction::Cancel { id } => {
            let id = match parse_id(&id) {
                Ok(id) => id,
                Err(code) => return code,
            };
            match client.send(Request::CancelQueued { id }).await {
                Ok(_) => {
                    if !json {
                        println!("removed from queue");
                    }
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
        QueueAction::CancelProcessing { id } => {
            let id = match parse_id(&id) {
                Ok(id) => id,
                Err(code) => return code,
            };
            match client.send(Request::CancelProcessing { id }).await {
                Ok(_) => {
                    if !json {
                        println!("cancelled in-flight item");
                    }
                    ExitCode::SUCCESS
                }
                Err(code) => code,
            }
        }
        QueueAction::CancelAll => match client.send(Request::CancelAllQueued).await {
            Ok(value) => {
                if json {
                    output::print_json(&value);
                } else {
                    let removed = value.get("removed").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("cleared {removed} pending item(s)");
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        QueueAction::ClearFailed => match client.send(Request::ClearFailed).await {
            Ok(value) => {
                if json {
                    output::print_json(&value);
                } else {
                    let removed = value.get("removed").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("dismissed {removed} failed item(s)");
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
    }
}

async fn set_paused(client: &mut Client, paused: bool, json: bool) -> ExitCode {
    match client.send(Request::SetQueuePaused { paused }).await {
        Ok(value) => {
            if json {
                output::print_json(&value);
            } else {
                println!("queue {}", if paused { "paused" } else { "resumed" });
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// Parse one id argument, printing a clear error and returning a failure exit
/// code on a malformed id (matching the rest of the CLI's id handling).
fn parse_id(id: &str) -> Result<RecordingId, ExitCode> {
    RecordingId::parse(id).ok_or_else(|| {
        eprintln!("error: '{id}' is not a valid recording id");
        ExitCode::FAILURE
    })
}

fn parse_ids(ids: &[String]) -> Result<Vec<RecordingId>, ExitCode> {
    ids.iter().map(|s| parse_id(s)).collect()
}

/// Render the queue array (in flight first, then pending) as a table.
fn print_queue_table(value: &serde_json::Value) {
    let Some(items) = value.as_array() else {
        println!("queue empty");
        return;
    };
    if items.is_empty() {
        println!("queue empty");
        return;
    }
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["state", "id", "dur", "model"]);
    for item in items {
        let state = item.get("state").and_then(|v| v.as_str()).unwrap_or("");
        let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let dur = item
            .get("duration_ms")
            .and_then(|v| v.as_i64())
            .map(output::format_duration)
            .unwrap_or_default();
        let model = item.get("model").and_then(|v| v.as_str()).unwrap_or("");
        table.add_row(vec![state, id, &dur, model]);
    }
    println!("{table}");
}
