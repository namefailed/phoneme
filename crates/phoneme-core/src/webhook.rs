use crate::error::{Error, Result};
use crate::types::HookPayload;
use std::time::Duration;

#[derive(Clone)]
pub struct WebhookClient {
    http: reqwest::Client,
}

impl WebhookClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder().build().map_err(|e| {
            crate::error::Error::Internal(format!("Failed to build reqwest client: {e}"))
        })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HookMetadata;
    use crate::RecordingId;
    use chrono::Local;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_payload() -> HookPayload {
        HookPayload {
            id: RecordingId::new(),
            timestamp: Local::now(),
            transcript: "hello world".into(),
            audio_path: "C:/tmp/x.wav".into(),
            duration_ms: 1234,
            model: "test-model".into(),
            metadata: HookMetadata::current(),
        }
    }

    /// A 2xx response is success, and the client POSTs the payload exactly once
    /// to the given URL (verified by the `.expect(1)` on drop).
    #[tokio::test]
    async fn post_succeeds_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let url = format!("{}/hook", server.uri());
        client
            .post(&url, Duration::from_secs(5), &sample_payload())
            .await
            .expect("2xx must be Ok");
    }

    /// A non-2xx response maps to `HookFailed` carrying the status code and the
    /// response body (so the failure surfaces a useful reason).
    #[tokio::test]
    async fn post_maps_non_2xx_to_hook_failed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream boom"))
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let err = client
            .post(&server.uri(), Duration::from_secs(5), &sample_payload())
            .await
            .expect_err("500 must be an error");
        match err {
            Error::HookFailed { code, stderr_tail } => {
                assert_eq!(code, 500);
                assert!(
                    stderr_tail.contains("upstream boom"),
                    "body should be carried, got: {stderr_tail}"
                );
            }
            other => panic!("expected HookFailed, got {other:?}"),
        }
    }

    /// A response slower than the per-request timeout maps to `HookTimeout`
    /// (not a generic error), so callers can distinguish a slow endpoint.
    #[tokio::test]
    async fn post_maps_timeout_to_hook_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let err = client
            .post(&server.uri(), Duration::from_millis(200), &sample_payload())
            .await
            .expect_err("a response slower than the timeout must error");
        assert!(
            matches!(err, Error::HookTimeout { .. }),
            "expected HookTimeout, got {err:?}"
        );
    }

    /// An unreachable endpoint maps to a (non-timeout) error rather than hanging
    /// or panicking.
    #[tokio::test]
    async fn post_unreachable_host_errors() {
        let client = WebhookClient::new().unwrap();
        // Reserved TEST-NET-1 address that should not accept connections.
        let err = client
            .post(
                "http://192.0.2.1:9/hook",
                Duration::from_secs(2),
                &sample_payload(),
            )
            .await
            .expect_err("unreachable host must error");
        // Either a connect error (Internal) or a timeout — both are acceptable;
        // the contract is "returns an error, doesn't hang/panic".
        assert!(matches!(
            err,
            Error::Internal(_) | Error::HookTimeout { .. }
        ));
    }
}
