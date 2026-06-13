//! `phoneme suggest-tags <ID>` — re-run the LLM tag-suggestion step on a
//! recording on demand (the CLI face of the GUI ✨ Suggest button).
//!
//! Spawning path, beside `cleanup` / `summarize`. Sends `SuggestTags`, which —
//! unlike the other LLM re-runs — awaits the model: the Ok reply arrives after
//! the suggestions land on the recording (`TagSuggestionsUpdated` fires for
//! open GUI views). Review the result with
//! `phoneme tag suggestions <ID>`. Errors when the recording has no transcript
//! yet (`invalid_config`) or the id is unknown (`not_found`).

use crate::args::SuggestTagsArgs;
use crate::client::Client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: SuggestTagsArgs, cfg: &Config, json: bool) -> ExitCode {
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

    match client.send(Request::SuggestTags { id }).await {
        Ok(_) => {
            if !json {
                println!("tag suggestions generated (review with `phoneme tag suggestions <id>`)");
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
    async fn sends_suggest_tags() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("suggest-tags", |_req| Response::Ok(serde_json::Value::Null));
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(SuggestTagsArgs { id: id.to_string() }, &cfg, false),
        )
        .await
        .expect("suggest-tags must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(mock.received(), vec![Request::SuggestTags { id }]);
    }
}
