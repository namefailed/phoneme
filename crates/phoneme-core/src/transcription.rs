use crate::error::{Error, Result};
use reqwest::multipart;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use tokio::fs;

/// HTTP client for an OpenAI-compatible `/v1/audio/transcriptions` endpoint.
#[derive(Debug, Clone)]
pub struct TranscriptionClient {
    base_url: String,
    timeout: Duration,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    text: String,
}

impl TranscriptionClient {
    pub fn new(base_url: String, timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client builds");
        Self { base_url, timeout, http }
    }

    /// POST the audio file as `multipart/form-data` and return the transcript.
    pub async fn transcribe(&self, audio_path: &Path) -> Result<String> {
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

        let url = format!("{}/v1/audio/transcriptions", self.base_url.trim_end_matches('/'));
        let response = match self.http.post(&url).multipart(form).send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(Error::LlmTimeout { secs: self.timeout.as_secs() })
            }
            Err(e) => return Err(Error::LlmUnreachable { url, source: e }),
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::LlmError { status: status.as_u16(), body });
        }

        let parsed: OpenAiResponse = response.json().await.map_err(|e| {
            Error::Internal(format!("decoding transcription response: {e}"))
        })?;
        Ok(parsed.text)
    }
}
