//! Behaviour spec for the LLM post-processing provider abstraction.
//!
//! Written test-first: defines how `LlmPostProcessor` selects a provider from
//! config and how each provider talks to its backend. Mirrors the transcription
//! provider tests (wiremock locks in request/response shape).

use phoneme_core::config::LlmPostProcessConfig;
use phoneme_core::{Error, LlmPostProcessor};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build an enabled post-process config for `provider`, pointed at `api_url`.
fn cfg(provider: &str, api_url: &str, api_key: &str, model: &str) -> LlmPostProcessConfig {
    LlmPostProcessConfig {
        enabled: true,
        provider: provider.to_string(),
        api_key: secrecy::SecretString::from(api_key.to_string()),
        api_url: api_url.to_string(),
        model: model.to_string(),
        prompt: "Clean this up".to_string(),
        timeout_secs: 5,
        num_ctx: 8192,
        autostart_ollama: true,
    }
}

#[tokio::test]
async fn ollama_provider_processes_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        // prompt + text are combined into the Ollama `prompt` field.
        .and(body_string_contains("Clean this up"))
        // the context window is capped via options.num_ctx to the configured
        // value (8192) so Ollama doesn't reserve a KV cache for the model's full
        // 128k window (~16 GiB). Match the VALUE, not just the key name — a
        // regression sending num_ctx:0 or a hardcoded wrong cap would still carry
        // the key, but not the configured 8192.
        .and(body_string_contains("\"num_ctx\":8192"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"response": "cleaned"})),
        )
        .mount(&server)
        .await;

    let pp = LlmPostProcessor::new().unwrap();
    let provider = pp
        .provider(&cfg(
            "ollama",
            &format!("{}/api/generate", server.uri()),
            "",
            "llama3.2:3b",
        ))
        .expect("ollama provider built");
    let out = provider.process("Clean this up", "raw text").await.unwrap();
    assert_eq!(out, "cleaned");
}

#[tokio::test]
async fn openai_chat_provider_sends_bearer_and_parses_choices() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "fixed"}}]
        })))
        .mount(&server)
        .await;

    let pp = LlmPostProcessor::new().unwrap();
    let provider = pp
        .provider(&cfg(
            "openai",
            &format!("{}/v1/chat/completions", server.uri()),
            "sk-test",
            "gpt-4o-mini",
        ))
        .expect("openai provider built");
    let out = provider.process("Clean this up", "raw").await.unwrap();
    assert_eq!(out, "fixed");
}

#[tokio::test]
async fn groq_routes_through_openai_chat_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer gsk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "groqed"}}]
        })))
        .mount(&server)
        .await;

    let pp = LlmPostProcessor::new().unwrap();
    let provider = pp
        .provider(&cfg(
            "groq",
            &format!("{}/v1/chat/completions", server.uri()),
            "gsk-test",
            "llama-3.1-8b-instant",
        ))
        .expect("groq provider built");
    let out = provider.process("Clean this up", "raw").await.unwrap();
    assert_eq!(out, "groqed");
}

#[tokio::test]
async fn anthropic_provider_sends_headers_and_parses_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "ak-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{"type": "text", "text": "claude output"}]
        })))
        .mount(&server)
        .await;

    let pp = LlmPostProcessor::new().unwrap();
    let provider = pp
        .provider(&cfg(
            "anthropic",
            &format!("{}/v1/messages", server.uri()),
            "ak-test",
            "claude-3-5-haiku-latest",
        ))
        .expect("anthropic provider built");
    let out = provider.process("Clean this up", "raw").await.unwrap();
    assert_eq!(out, "claude output");
}

#[tokio::test]
async fn disabled_or_none_yields_no_provider() {
    let pp = LlmPostProcessor::new().unwrap();

    let mut disabled = cfg("openai", "", "k", "m");
    disabled.enabled = false;
    assert!(pp.provider(&disabled).is_none(), "disabled => no provider");

    assert!(
        pp.provider(&cfg("none", "", "", "")).is_none(),
        "provider=none => no provider"
    );
    assert!(
        pp.provider(&cfg("not-a-provider", "", "", "")).is_none(),
        "unknown provider => no provider (lenient passthrough)"
    );
}

#[tokio::test]
async fn non_success_status_is_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let pp = LlmPostProcessor::new().unwrap();
    let provider = pp
        .provider(&cfg(
            "openai",
            &format!("{}/v1/chat/completions", server.uri()),
            "k",
            "m",
        ))
        .expect("provider built");
    let err = provider.process("p", "t").await.unwrap_err();
    assert!(matches!(err, Error::Internal(_)), "got {err:?}");
}
