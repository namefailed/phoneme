//! LLM post-processing providers.
//!
//! Mirrors the transcription provider design: an [`LlmProvider`] trait with one
//! impl per backend, selected from `[llm_post_process]` config by
//! [`LlmPostProcessor`], which owns one warm `reqwest::Client` and mints a
//! `Box<dyn LlmProvider>` per pipeline run. `provider()` returns `None` when
//! post-processing is disabled or the provider is `none`/unrecognized — the
//! pipeline treats that as "no post-processing".
//!
//! `OpenAiChatProvider` serves any OpenAI-compatible chat-completions endpoint
//! (OpenAI, Groq, LM Studio, Jan, ...). Ollama and Anthropic have their own
//! wire formats. All errors map to `Error::Internal`: post-processing is
//! non-fatal, so the pipeline logs and falls back to the raw transcript.

use crate::config::LlmPostProcessConfig;
use crate::error::{Error, Result};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::Deserialize;
use std::time::Duration;

/// Post-processes a transcript with an LLM.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Apply the instruction `prompt` to `text`, returning the new text.
    async fn process(&self, prompt: &str, text: &str) -> Result<String>;
}

/// Owns the process-wide HTTP client and builds an [`LlmProvider`] per request
/// from `[llm_post_process]` config, sharing one warm connection pool.
#[derive(Debug, Clone)]
pub struct LlmPostProcessor {
    http: reqwest::Client,
}

impl LlmPostProcessor {
    /// Create a post-processor with a fresh shared HTTP client.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Internal(format!("Failed to build reqwest client: {e}")))?;
        Ok(Self { http })
    }

    /// Build the configured provider, or `None` when post-processing is
    /// disabled or the provider is `none`/unrecognized (lenient passthrough).
    pub fn provider(&self, cfg: &LlmPostProcessConfig) -> Option<Box<dyn LlmProvider>> {
        if !cfg.enabled {
            return None;
        }
        let timeout = Duration::from_secs(cfg.timeout_secs);
        match cfg.provider.trim().to_ascii_lowercase().as_str() {
            "ollama" => Some(Box::new(OllamaProvider {
                http: self.http.clone(),
                url: non_empty_or(&cfg.api_url, "http://127.0.0.1:11434/api/generate"),
                model: non_empty_or(&cfg.model, "llama3.2:3b"),
                timeout,
            })),
            "openai" => Some(Box::new(OpenAiChatProvider {
                http: self.http.clone(),
                url: non_empty_or(&cfg.api_url, "https://api.openai.com/v1/chat/completions"),
                api_key: cfg.api_key.expose_secret().trim().to_string(),
                model: non_empty_or(&cfg.model, "gpt-4o-mini"),
                timeout,
            })),
            "groq" => Some(Box::new(OpenAiChatProvider {
                http: self.http.clone(),
                url: non_empty_or(
                    &cfg.api_url,
                    "https://api.groq.com/openai/v1/chat/completions",
                ),
                api_key: cfg.api_key.expose_secret().trim().to_string(),
                model: non_empty_or(&cfg.model, "llama-3.1-8b-instant"),
                timeout,
            })),
            "anthropic" => Some(Box::new(AnthropicProvider {
                http: self.http.clone(),
                url: non_empty_or(&cfg.api_url, "https://api.anthropic.com/v1/messages"),
                api_key: cfg.api_key.expose_secret().trim().to_string(),
                model: non_empty_or(&cfg.model, "claude-3-5-haiku-latest"),
                timeout,
            })),
            _ => None,
        }
    }
}

/// The trimmed `value`, or `default` if it is empty/whitespace.
fn non_empty_or(value: &str, default: &str) -> String {
    let v = value.trim();
    if v.is_empty() {
        default.to_string()
    } else {
        v.to_string()
    }
}

/// Combine the instruction prompt and transcript into one user message.
fn combine(prompt: &str, text: &str) -> String {
    format!("{prompt}:\n{text}")
}

/// Normalize LLM response by collapsing multiple consecutive newlines into single newlines
/// and trimming excessive whitespace while preserving paragraph structure.
fn normalize_response(text: &str) -> String {
    // Replace 3+ consecutive newlines with exactly 2 newlines (preserve paragraph breaks)
    let collapsed = regex::Regex::new(r"\n{3,}").unwrap().replace_all(text, "\n\n");
    // Trim leading/trailing whitespace
    collapsed.trim().to_string()
}

/// Send a request and decode its JSON body. Every failure (transport, non-2xx,
/// or decode) maps to `Error::Internal` with a `who`-prefixed message.
async fn send_json<T: serde::de::DeserializeOwned>(
    req: reqwest::RequestBuilder,
    who: &str,
) -> Result<T> {
    let resp = req
        .send()
        .await
        .map_err(|e| Error::Internal(format!("{who} request failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Internal(format!(
            "{who} returned status {status}: {body}"
        )));
    }
    resp.json::<T>()
        .await
        .map_err(|e| Error::Internal(format!("decoding {who} response: {e}")))
}

// ── Ollama (POST /api/generate) ───────────────────────────────────────────────

#[derive(Debug, Clone)]
struct OllamaProvider {
    http: reqwest::Client,
    url: String,
    model: String,
    timeout: Duration,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn process(&self, prompt: &str, text: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "prompt": combine(prompt, text),
            "stream": false,
        });
        let parsed: OllamaResponse = send_json(
            self.http.post(&self.url).timeout(self.timeout).json(&body),
            "Ollama",
        )
        .await?;
        Ok(normalize_response(&parsed.response))
    }
}

// ── OpenAI-compatible chat completions (OpenAI, Groq, LM Studio, ...) ──────────

#[derive(Clone)]
struct OpenAiChatProvider {
    http: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    timeout: Duration,
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for OpenAiChatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiChatProvider")
            .field("http", &self.http)
            .field("url", &self.url)
            .field("api_key", &crate::config::redact_key(&self.api_key))
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: String,
}

#[async_trait]
impl LlmProvider for OpenAiChatProvider {
    async fn process(&self, prompt: &str, text: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{ "role": "user", "content": combine(prompt, text) }],
            "temperature": 0.3,
        });
        let mut req = self.http.post(&self.url).timeout(self.timeout).json(&body);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let parsed: OpenAiChatResponse = send_json(req, "OpenAI-compatible").await?;
        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| normalize_response(&c.message.content))
            .ok_or_else(|| Error::Internal("OpenAI-compatible response had no choices".into()))
    }
}

// ── Anthropic Claude (POST /v1/messages) ───────────────────────────────────────

#[derive(Clone)]
struct AnthropicProvider {
    http: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    timeout: Duration,
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("http", &self.http)
            .field("url", &self.url)
            .field("api_key", &crate::config::redact_key(&self.api_key))
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicBlock>,
    /// "end_turn", "max_tokens", "stop_sequence", … — used to detect truncation.
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicBlock {
    /// Present on `text` blocks; absent on other block types (tool_use, etc.).
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn process(&self, prompt: &str, text: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 8192,
            "messages": [{ "role": "user", "content": combine(prompt, text) }],
        });
        let req = self
            .http
            .post(&self.url)
            .timeout(self.timeout)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body);
        let parsed: AnthropicResponse = send_json(req, "Anthropic").await?;
        // Don't return a transcript Claude truncated at max_tokens — fail so the
        // (non-fatal) post-processing step falls back to the raw transcript.
        if parsed.stop_reason.as_deref() == Some("max_tokens") {
            return Err(Error::Internal(
                "Anthropic response truncated at max_tokens".into(),
            ));
        }
        parsed
            .content
            .into_iter()
            .find_map(|b| b.text)
            .map(|t| normalize_response(&t))
            .ok_or_else(|| Error::Internal("Anthropic response had no text content".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The provider structs hold a plaintext API key but must never render it in
    // `Debug` output (which can reach logs / panic backtraces).
    #[test]
    fn openai_chat_provider_debug_redacts_api_key() {
        let p = OpenAiChatProvider {
            http: reqwest::Client::new(),
            url: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: "sk-SUPER-SECRET-12345".to_string(),
            model: "gpt-4o".to_string(),
            timeout: Duration::from_secs(30),
        };
        let dbg = format!("{p:?}");
        assert!(
            !dbg.contains("sk-SUPER-SECRET-12345"),
            "api key leaked: {dbg}"
        );
        assert!(dbg.contains("redacted"));
    }

    #[test]
    fn anthropic_provider_debug_redacts_api_key() {
        let p = AnthropicProvider {
            http: reqwest::Client::new(),
            url: "https://api.anthropic.com/v1/messages".to_string(),
            api_key: "sk-ant-SUPER-SECRET-67890".to_string(),
            model: "claude-3-haiku".to_string(),
            timeout: Duration::from_secs(30),
        };
        let dbg = format!("{p:?}");
        assert!(
            !dbg.contains("sk-ant-SUPER-SECRET-67890"),
            "api key leaked: {dbg}"
        );
        assert!(dbg.contains("redacted"));
    }

    #[test]
    fn non_empty_or_falls_back_when_blank() {
        assert_eq!(non_empty_or("   ", "def"), "def");
        assert_eq!(non_empty_or("", "def"), "def");
        assert_eq!(non_empty_or("  x ", "def"), "x");
    }

    #[test]
    fn combine_joins_prompt_and_text() {
        assert_eq!(combine("Fix", "hello"), "Fix:\nhello");
    }
}
