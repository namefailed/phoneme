//! Wizard-only backend helpers.

use crate::bridge::Bridge;
use phoneme_ipc::{Request, Response};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TestConnectResult {
    pub ok: bool,
    pub message: String,
}

/// Test that `cfg.llm.external_url` responds. Best-effort GET probe.
pub async fn test_llm_endpoint(url: &str) -> TestConnectResult {
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

/// Run the configured hook with a sample payload via the daemon.
pub async fn test_hook(bridge: Option<&Bridge>) -> TestConnectResult {
    let Some(bridge) = bridge else {
        return TestConnectResult {
            ok: false,
            message: "daemon not reachable".into(),
        };
    };
    match bridge.request(Request::HookTest).await {
        Ok(Response::Ok(v)) => TestConnectResult {
            ok: v["exit_code"].as_i64() == Some(0),
            message: format!("exit {} in {}ms", v["exit_code"], v["duration_ms"]),
        },
        Ok(Response::Err(e)) => TestConnectResult {
            ok: false,
            message: e.message,
        },
        Err(e) => TestConnectResult {
            ok: false,
            message: e.to_string(),
        },
    }
}
