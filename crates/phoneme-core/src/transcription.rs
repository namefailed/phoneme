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
    /// Creates a new `TranscriptionClient` equipped with an internal HTTP client.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder().build().map_err(|e| {
            crate::error::Error::Internal(format!("Failed to build reqwest client: {e}"))
        })?;
        Ok(Self { http })
    }

    /// Transcribes an audio file by submitting a `multipart/form-data` request
    /// to an OpenAI-compatible `/v1/audio/transcriptions` endpoint.
    ///
    /// # Arguments
    /// * `base_url` - The base URL of the whisper server (e.g. `http://127.0.0.1:8080`)
    /// * `timeout` - Maximum duration to wait for the transcription to complete
    /// * `audio_path` - Path to the `.wav` file on disk to be transcribed
    /// * `language` - Optional BCP-47 language code hint (e.g. `"en"`, `"es"`).
    ///   `None` means auto-detect.
    ///
    /// # Returns
    /// The transcribed text string on success. Returns `Error::WhisperTimeout` or
    /// `Error::WhisperUnreachable` on network issues, and `Error::WhisperError` if the
    /// API responds with a non-success HTTP status.
    pub async fn transcribe(
        &self,
        base_url: &str,
        timeout: Duration,
        audio_path: &Path,
        language: Option<&str>,
    ) -> Result<String> {
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
        let mut form = multipart::Form::new().part("file", part);
        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }

        let url = format!("{}/v1/audio/transcriptions", base_url.trim_end_matches('/'));
        let response = match self
            .http
            .post(&url)
            .timeout(timeout)
            .multipart(form)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(Error::WhisperTimeout {
                    secs: timeout.as_secs(),
                })
            }
            Err(e) => return Err(Error::WhisperUnreachable { url, source: e }),
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::WhisperError {
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
