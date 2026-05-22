use crate::error::{Error, Result};
use crate::types::HookPayload;
use std::time::Duration;

pub struct WebhookClient {
    url: String,
    timeout: Duration,
    http: reqwest::Client,
}

impl WebhookClient {
    pub fn new(url: String, timeout: Duration) -> Self {
        let http = reqwest::Client::builder().timeout(timeout).build().unwrap();
        Self { url, timeout, http }
    }

    pub async fn post(&self, payload: &HookPayload) -> Result<()> {
        let response = self
            .http
            .post(&self.url)
            .json(payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    Error::HookTimeout { secs: self.timeout.as_secs() }
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
