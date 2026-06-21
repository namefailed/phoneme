//! `phoneme chapters <ID>` — generate a recording's topic chapters on demand and
//! print them (the CLI face of the Chapters detail view + the ✨ Generate button).
//!
//! Sends `SuggestChapters` (awaits the model, like `suggest-entities`), then
//! `GetChapters` to print the resulting time-coded chapter list. `--show` skips
//! generation and just prints the stored chapters. Errors when the recording has
//! no transcript/segments to chapter (`invalid_config`) or the id is unknown
//! (`not_found`); a recording with no timing simply prints an empty list.

use crate::args::ChaptersArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Chapter, Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: ChaptersArgs, cfg: &Config, json: bool) -> ExitCode {
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

    // Generate first (awaits the model, like `suggest-entities`), unless `--show`
    // asks only to view what's already stored.
    if !args.show {
        if let Err(code) = client
            .send(Request::SuggestChapters { id: id.clone() })
            .await
        {
            return code;
        }
    }

    let value = match client.send(Request::GetChapters { id: id.clone() }).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let chapters: Vec<Chapter> = match serde_json::from_value(value) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: parsing chapters: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };

    if json {
        output::print_json_lines(&chapters);
    } else if chapters.is_empty() {
        println!("no chapters (the recording may have no timing to chapter)");
    } else {
        for c in &chapters {
            let mins = c.start_ms / 60_000;
            let secs = (c.start_ms % 60_000) / 1_000;
            match c.summary.as_deref() {
                Some(sum) if !sum.is_empty() => {
                    println!("{mins:02}:{secs:02}  {}  — {sum}", c.title)
                }
                _ => println!("{mins:02}:{secs:02}  {}", c.title),
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    #[tokio::test]
    async fn generates_then_prints_chapters() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("chapters", |req| match req {
            Request::SuggestChapters { .. } => Response::Ok(serde_json::Value::Null),
            Request::GetChapters { .. } => Response::Ok(serde_json::json!([
                { "start_ms": 0, "end_ms": 60000, "title": "Intro", "summary": "kickoff" },
                { "start_ms": 60000, "end_ms": 120000, "title": "Deep dive", "summary": null },
            ])),
            _ => Response::Ok(serde_json::Value::Null),
        });
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(
                ChaptersArgs {
                    id: id.to_string(),
                    show: false,
                },
                &cfg,
                false,
            ),
        )
        .await
        .expect("chapters must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            mock.received(),
            vec![
                Request::SuggestChapters { id: id.clone() },
                Request::GetChapters { id },
            ]
        );
    }

    #[tokio::test]
    async fn show_skips_generation() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("chapters", |_req| Response::Ok(serde_json::json!([])));
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(
                ChaptersArgs {
                    id: id.to_string(),
                    show: true,
                },
                &cfg,
                false,
            ),
        )
        .await
        .expect("chapters --show must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        // --show fetches only; it never sends SuggestChapters.
        assert_eq!(mock.received(), vec![Request::GetChapters { id }]);
    }
}
