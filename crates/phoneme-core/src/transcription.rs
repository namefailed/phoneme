use crate::error::{Error, Result};
use reqwest::multipart;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use tokio::fs;

/// HTTP client for an OpenAI-compatible `/v1/audio/transcriptions` endpoint.
#[derive(Debug, Clone)]
pub struct TranscriptionClient {
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    text: String,
}

impl TranscriptionClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .build()
            .expect("reqwest client builds");
        Self { http }
    }

    pub async fn transcribe(&self, base_url: &str, timeout: Duration, audio_path: &Path) -> Result<String> {
        let bytes = fs::read(audio_path).await?;
        let part = multipart::Part::bytes(bytes)
            .file_name(
                audio_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("audio.wav")
                    .to_string(),
            )
            .mime_str("audio/wav")
            .map_err(|e| Error::Internal(format!("multipart mime: {e}")))?;
        let form = multipart::Form::new().part("file", part);

        let url = format!(
            "{}/inference",
            base_url.trim_end_matches('/')
        );
        let response = match self.http.post(&url).timeout(timeout).multipart(form).send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(Error::LlmTimeout {
                    secs: timeout.as_secs(),
                })
            }
            Err(e) => return Err(Error::LlmUnreachable { url, source: e }),
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::LlmError {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("decoding transcription response: {e}")))?;
        Ok(parsed.text)
    }
}
