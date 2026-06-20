//! Wizard-only backend helpers — connection tests for the first-run wizard
//! and the Settings "test" buttons.
//!
//! `test_whisper_endpoint` GETs an external whisper URL (any HTTP answer, even a
//! 4xx, proves something is listening and speaking HTTP) and returns the
//! WebView-friendly [`TestConnectResult`] `{ok, message}` instead of an error
//! type. The wizard's download commands live in `commands` (they need the Tauri
//! app handle) with their integrity pins in `checksums`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TestConnectResult {
    pub ok: bool,
    pub message: String,
}

/// Test that `cfg.whisper.external_url` responds. Best-effort GET probe.
pub async fn test_whisper_endpoint(url: &str) -> TestConnectResult {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return TestConnectResult {
                ok: false,
                message: format!("client build failed: {e}"),
            }
        }
    };
    match client.get(url).send().await {
        Ok(r) => TestConnectResult {
            // A 4xx still proves the endpoint is reachable and speaking HTTP.
            ok: r.status().is_success() || r.status().is_client_error(),
            message: format!("HTTP {}", r.status()),
        },
        Err(e) => TestConnectResult {
            ok: false,
            message: format!("{e}"),
        },
    }
}
