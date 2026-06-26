//! `phoneme versions <ID>` — list a recording's transcript-version chain (raw ASR
//! → each pipeline step → live) for side-by-side comparison. The CLI face of the
//! Compare-versions view, and a cross-platform alternative to the daemon named
//! pipe — the only other surface that exposes versions, so clients on macOS/Linux
//! (and the REST bridge) couldn't reach them before.
//!
//! Sends `ListTranscriptVersions`; prints the chain in `idx` order. `--json` emits
//! the raw array; otherwise one `idx  label  (model)` line per version.

use crate::args::IdArgs;
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: IdArgs, cfg: &Config, json: bool) -> ExitCode {
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

    let value = match client.send(Request::ListTranscriptVersions { id }).await {
        Ok(v) => v,
        Err(code) => return code,
    };

    if json {
        output::print_json(&value);
        return ExitCode::SUCCESS;
    }

    let versions = value.as_array().cloned().unwrap_or_default();
    if versions.is_empty() {
        println!("no transcript versions stored for this recording");
        return ExitCode::SUCCESS;
    }
    for v in &versions {
        let idx = v.get("idx").and_then(|x| x.as_i64()).unwrap_or(0);
        let label = v
            .get("label")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if idx == 0 {
                    "Raw (ASR)".into()
                } else {
                    format!("Step {idx}")
                }
            });
        let model = v.get("model").and_then(|x| x.as_str()).unwrap_or("");
        if model.is_empty() {
            println!("{idx:>2}  {label}");
        } else {
            println!("{idx:>2}  {label}  ({model})");
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
    async fn lists_versions() {
        let id = RecordingId::new();
        let mock = MockDaemon::spawn("versions", |req| match req {
            Request::ListTranscriptVersions { .. } => Response::Ok(serde_json::json!([
                { "idx": 0, "label": null, "model": null, "text": "raw" },
                { "idx": 1, "label": "Cleanup (llama3.2)", "model": "llama3.2", "text": "clean" },
            ])),
            _ => Response::Ok(serde_json::Value::Null),
        });
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();

        let code = tokio::time::timeout(
            Duration::from_secs(5),
            run(IdArgs { id: id.to_string() }, &cfg, false),
        )
        .await
        .expect("versions must return promptly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            mock.received(),
            vec![Request::ListTranscriptVersions { id }]
        );
    }
}
