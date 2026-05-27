//! Pipeline orchestration: transcribe → hook → done.
//!
//! Called by the queue worker per claimed payload.

use crate::app_state::AppState;
use phoneme_core::error::Result;
use phoneme_core::{HookMetadata, HookPayload, HookRunner, RecordingStatus};
use phoneme_ipc::DaemonEvent;
use std::time::Duration;

/// Process a single claimed payload through the full pipeline.
///
/// Updates catalog, fires events, moves inbox files to done/ or failed/.
pub async fn run(state: &AppState, mut payload: HookPayload) -> Result<()> {
    let id = payload.id.clone();
    state
        .events
        .emit(DaemonEvent::TranscriptionStarted { id: id.clone() });

    // Transcribe — reuse the process-wide client (AppState) so the HTTP
    // connection pool to the local whisper-server stays warm across items.
    let cfg = state.config.load();
    let audio_path = std::path::Path::new(&payload.audio_path).to_path_buf();
    // Filter empty string to None — frontend sends "" for "auto-detect"
    let language = cfg.whisper.language.clone().filter(|s| !s.is_empty());
    let transcript = match state
        .transcription
        .transcribe(
            &cfg.whisper.external_url,
            Duration::from_secs(cfg.whisper.timeout_secs),
            &audio_path,
            language.as_deref(),
        )
        .await
    {
        Ok(t) => t,
        Err(e) => {
            state
                .catalog
                .update_status(&id, RecordingStatus::TranscribeFailed)
                .await?;
            state
                .inbox
                .finish_failed(&id, "whisper_error", &e.to_string())
                .await?;
            state.events.emit(DaemonEvent::TranscriptionFailed {
                id: id.clone(),
                error: e.to_string(),
            });
            return Err(e);
        }
    };

    let mut transcript = transcript;
    if cfg.llm_post_process.enabled {
        match post_process_transcript(&cfg.llm_post_process, &transcript, None).await {
            Ok(processed) => {
                tracing::info!("LLM post-processing succeeded!");
                transcript = processed;
            }
            Err(e) => {
                tracing::error!(error = %e, "LLM post-processing failed, falling back to raw transcript");
            }
        }
    }

    payload.transcript = transcript.clone();
    // The whisper-server supervisor (Task 12) will publish the actually-loaded
    // model name; until then, fall back to the configured model_path's file
    // stem or "unknown".
    payload.model = std::path::Path::new(&cfg.whisper.model_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    state
        .catalog
        .update_transcript(&id, &transcript, &payload.model)
        .await?;
    state
        .catalog
        .update_status(&id, RecordingStatus::HookRunning)
        .await?;
    state.events.emit(DaemonEvent::TranscriptionDone {
        id: id.clone(),
        transcript: transcript.clone(),
    });

    // Hooks.
    state
        .events
        .emit(DaemonEvent::HookStarted { id: id.clone() });
    payload.metadata = HookMetadata::current();

    let mut final_exit_code = 0;
    let mut total_duration = 0;
    let mut last_cmd = String::new();

    let expanded_cfg = cfg.expanded().unwrap_or_else(|_| (**cfg).clone());

    for cmd in &expanded_cfg.hook.commands {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            continue;
        }
        let runner = HookRunner::new(
            trimmed.to_string(),
            Duration::from_secs(cfg.hook.timeout_secs),
        );
        match runner.run(&payload).await {
            Ok(result) => {
                final_exit_code = result.exit_code;
                total_duration += result.duration_ms;
                last_cmd = cmd.clone();
                if result.exit_code != 0 {
                    break;
                }
            }
            Err(e) => {
                state
                    .catalog
                    .update_status(&id, RecordingStatus::HookFailed)
                    .await?;
                state
                    .inbox
                    .finish_failed(&id, "hook_failed", &e.to_string())
                    .await?;
                state.events.emit(DaemonEvent::HookFailed {
                    id,
                    error: e.to_string(),
                });
                return Err(e);
            }
        }
    }

    state
        .catalog
        .update_hook_result(&id, &last_cmd, final_exit_code, total_duration)
        .await?;
    state
        .catalog
        .update_status(&id, RecordingStatus::Done)
        .await?;
    state.inbox.finish_done(&id, &payload).await?;
    state.events.emit(DaemonEvent::HookDone {
        id,
        exit_code: final_exit_code,
    });

    if let Some(url) = &cfg.hook.webhook_url {
        if let Err(e) = state
            .webhook
            .post(url, Duration::from_secs(cfg.hook.timeout_secs), &payload)
            .await
        {
            tracing::warn!(error = %e, "webhook failed");
        }
    }

    Ok(())
}

async fn post_process_transcript(
    cfg: &phoneme_core::config::LlmPostProcessConfig,
    text: &str,
    base_url_override: Option<&str>,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    if cfg.provider == "ollama" {
        let default_url = "http://127.0.0.1:11434/api/generate";
        let mut url = if cfg.api_url.is_empty() {
            default_url
        } else {
            &cfg.api_url
        };
        if let Some(override_url) = base_url_override {
            url = override_url;
        }

        let body = serde_json::json!({
            "model": cfg.model,
            "prompt": format!("{}:\n{}", cfg.prompt, text),
            "stream": false
        });
        let res = client.post(url).json(&body).send().await?;
        if !res.status().is_success() {
            anyhow::bail!("Ollama returned non-success status: {}", res.status());
        }
        let parsed: serde_json::Value = res.json().await?;
        let output = parsed
            .get("response")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing response field in Ollama output"))?;
        return Ok(output.trim().to_string());
    } else if cfg.provider == "openai" {
        let default_url = "https://api.openai.com/v1/chat/completions";
        let mut url = if cfg.api_url.is_empty() {
            default_url
        } else {
            &cfg.api_url
        };
        if let Some(override_url) = base_url_override {
            url = override_url;
        }

        let body = serde_json::json!({
            "model": cfg.model,
            "messages": [
                { "role": "user", "content": format!("{}:\n{}", cfg.prompt, text) }
            ],
            "temperature": 0.3
        });
        let mut req = client.post(url).json(&body);
        if !cfg.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", cfg.api_key));
        }
        let res = req.send().await?;
        if !res.status().is_success() {
            let status = res.status();
            let err_body = res.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI returned status {}: {}", status, err_body);
        }
        let parsed: serde_json::Value = res.json().await?;
        let output = parsed
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|f| f.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| anyhow::anyhow!("Unexpected response format from OpenAI"))?;
        return Ok(output.trim().to_string());
    }

    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoneme_core::config::LlmPostProcessConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_post_process_openai_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "Fixed Transcript"
                    }
                }]
            })))
            .mount(&mock_server)
            .await;

        let cfg = LlmPostProcessConfig {
            enabled: true,
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
            api_key: "test-key".to_string(),
            api_url: "".to_string(),
            prompt: "Fix it".to_string(),
        };

        let url = format!("{}/v1/chat/completions", mock_server.uri());
        let result = post_process_transcript(&cfg, "Raw Transcript", Some(&url))
            .await
            .unwrap();
        assert_eq!(result, "Fixed Transcript");
    }

    #[tokio::test]
    async fn test_post_process_ollama_failure() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/generate"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let cfg = LlmPostProcessConfig {
            enabled: true,
            provider: "ollama".to_string(),
            model: "llama3".to_string(),
            api_key: "".to_string(),
            api_url: "".to_string(),
            prompt: "Fix it".to_string(),
        };

        let url = format!("{}/api/generate", mock_server.uri());
        let result = post_process_transcript(&cfg, "Raw Transcript", Some(&url)).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("non-success status"));
    }
}
