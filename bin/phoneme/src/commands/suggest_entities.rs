//! `phoneme suggest-entities <ID>` — run the LLM entity-extraction step on a
//! recording on demand (the CLI face of the GUI 🔎 Extract button).
//!
//! Spawning path, beside `suggest-tags` / `summarize`. Sends `SuggestEntities`,
//! which — like `SuggestTags` — awaits the model: the Ok reply arrives after the
//! structured entities land on the recording (`EntitiesUpdated` fires for open
//! GUI views). Review the result with `phoneme show <ID>`. Errors when the
//! recording has no transcript yet (`invalid_config`) or the id is unknown
//! (`not_found`).

use crate::args::SuggestEntitiesArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: SuggestEntitiesArgs, cfg: &Config, json: bool) -> ExitCode {
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

    match client.send(Request::SuggestEntities { id }).await {
        Ok(_) => {
            if !json {
                println!("entities extracted (review with `phoneme show <id>`)");
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
    async fn sends_suggest_entities() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("suggest-entities", |_req| {
            Response::Ok(serde_json::Value::Null)
        });
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(SuggestEntitiesArgs { id: id.to_string() }, &cfg, false),
        )
        .await
        .expect("suggest-entities must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(mock.received(), vec![Request::SuggestEntities { id }]);
    }
}
