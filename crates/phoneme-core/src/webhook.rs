use crate::error::{Error, Result};
use crate::types::HookPayload;
use std::time::Duration;

#[derive(Clone)]
pub struct WebhookClient {
    http: reqwest::Client,
}



impl WebhookClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| crate::error::Error::Internal(format!("Failed to build reqwest client: {e}")))?;
        Ok(Self { http })
    }

    pub async fn post(&self, url: &str, timeout: Duration, payload: &HookPayload) -> Result<()> {
        let response = self
            .http
            .post(url)
            .timeout(timeout)
            .json(payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    Error::HookTimeout {
                        secs: timeout.as_secs(),
                    }
                } else {
                    Error::Internal(format!("webhook send failed: {e}"))
                }
            })?;
        if !response.status().is_success() {
            return Err(Error::HookFailed {
                code: response.status().as_u16() as i32,
                stderr_tail: response.text().await.unwrap_or_default(),
            });
        }
        Ok(())
    }
}
