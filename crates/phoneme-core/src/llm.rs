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

/// A sink for streamed response tokens, passed to
/// [`LlmProvider::process_streaming`] to forward partial output to the UI as it
/// is produced. `Send` so it can cross `.await` points inside the async provider.
pub type DeltaSink<'a> = &'a mut (dyn FnMut(&str) + Send);

/// Post-processes a transcript with an LLM.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Apply the instruction `prompt` to `text`, returning the new text.
    async fn process(&self, prompt: &str, text: &str) -> Result<String>;

    /// The verbatim prompt this provider sends to the model for `(prompt,
    /// text)`, so the GUI can show exactly what was sent. The default matches
    /// every current provider (a single user message of `combine(...)`).
    fn exact_prompt(&self, prompt: &str, text: &str) -> String {
        combine(prompt, text)
    }

    /// Like [`process`](Self::process) but forwards partial response text to
    /// `on_delta` as it is produced. The default calls `process` and emits the
    /// whole result as one delta — correct for non-streaming providers
    /// (OpenAI-compatible, Groq, Anthropic). Ollama overrides this to stream.
    async fn process_streaming(
        &self,
        prompt: &str,
        text: &str,
        on_delta: DeltaSink<'_>,
    ) -> Result<String> {
        let out = self.process(prompt, text).await?;
        on_delta(&out);
        Ok(out)
    }
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
                url: non_empty_or(&cfg.api_url, crate::endpoints::OLLAMA_LLM_URL),
                model: non_empty_or(&cfg.model, "llama3.2:3b"),
                timeout,
            })),
            "openai" => Some(Box::new(OpenAiChatProvider {
                http: self.http.clone(),
                url: non_empty_or(&cfg.api_url, crate::endpoints::OPENAI_LLM_URL),
                api_key: cfg.api_key.expose_secret().trim().to_string(),
                model: non_empty_or(&cfg.model, "gpt-4o-mini"),
                timeout,
            })),
            "groq" => Some(Box::new(OpenAiChatProvider {
                http: self.http.clone(),
                url: non_empty_or(&cfg.api_url, crate::endpoints::GROQ_LLM_URL),
                api_key: cfg.api_key.expose_secret().trim().to_string(),
                model: non_empty_or(&cfg.model, "llama-3.1-8b-instant"),
                timeout,
            })),
            "anthropic" => Some(Box::new(AnthropicProvider {
                http: self.http.clone(),
                url: non_empty_or(&cfg.api_url, crate::endpoints::ANTHROPIC_LLM_URL),
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
/// and removing single newlines that break sentences (unless followed by sentence-ending punctuation).
fn normalize_response(text: &str) -> String {
    // First, collapse 3+ consecutive newlines into 2 newlines (preserve paragraph breaks)
    let collapsed = regex::Regex::new(r"\n{3,}")
        .unwrap()
        .replace_all(text, "\n\n");

    // Then, collapse a *single* newline that merely wraps a sentence. The
    // newline must be preceded by a non-newline, non-sentence-ending character
    // and followed by a lowercase letter. Requiring a non-newline char before
    // the newline leaves paragraph breaks (`\n\n`) intact (the previous
    // `\n([a-z])` ate the second newline of a pair); excluding `.?!` preserves a
    // newline that follows sentence-ending punctuation; the lowercase look-ahead
    // preserves a newline before a capitalized word.
    let sentence_normalized = regex::Regex::new(r"([^\n.!?])\n([a-z])")
        .unwrap()
        .replace_all(&collapsed, "${1} ${2}");

    // Trim leading/trailing whitespace
    sentence_normalized.trim().to_string()
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

/// One NDJSON object from a streaming `/api/generate` response. `response` is a
/// token/chunk; `done` marks the final object.
#[derive(Debug, Deserialize)]
struct OllamaStreamChunk {
    #[serde(default)]
    response: String,
    #[serde(default)]
    done: bool,
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

    /// Stream tokens from Ollama's NDJSON response, forwarding each chunk to
    /// `on_delta` as it arrives. The accumulated raw text is normalized once at
    /// the end so the stored result matches the non-streaming path.
    async fn process_streaming(
        &self,
        prompt: &str,
        text: &str,
        on_delta: DeltaSink<'_>,
    ) -> Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "prompt": combine(prompt, text),
            "stream": true,
        });
        let resp = self
            .http
            .post(&self.url)
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Ollama request failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Internal(format!(
                "Ollama returned status {status}: {body}"
            )));
        }

        // NDJSON: one JSON object per line. reqwest hands us arbitrary byte
        // chunks, so buffer and split on '\n' rather than assuming a chunk is a
        // whole line.
        let mut acc = String::new();
        let mut buf = String::new();
        let mut resp = resp;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| Error::Internal(format!("Ollama stream error: {e}")))?
        {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(obj) = serde_json::from_str::<OllamaStreamChunk>(line) {
                    if !obj.response.is_empty() {
                        acc.push_str(&obj.response);
                        on_delta(&obj.response);
                    }
                    if obj.done {
                        return Ok(normalize_response(&acc));
                    }
                }
            }
        }
        // Stream ended without an explicit done — flush any trailing line.
        let tail = buf.trim();
        if !tail.is_empty() {
            if let Ok(obj) = serde_json::from_str::<OllamaStreamChunk>(tail) {
                if !obj.response.is_empty() {
                    acc.push_str(&obj.response);
                    on_delta(&obj.response);
                }
            }
        }
        Ok(normalize_response(&acc))
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
            url: crate::endpoints::OPENAI_LLM_URL.to_string(),
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
            url: crate::endpoints::ANTHROPIC_LLM_URL.to_string(),
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

    #[test]
    fn normalize_response_collapses_excessive_newlines() {
        assert_eq!(normalize_response("hello\n\n\nworld"), "hello\n\nworld");
        assert_eq!(normalize_response("hello\n\n\n\n\nworld"), "hello\n\nworld");
    }

    #[test]
    fn normalize_response_collapses_single_newlines_breaking_sentences() {
        assert_eq!(normalize_response("hello\nworld"), "hello world");
        assert_eq!(
            normalize_response("testing a\ntranscription with\nthe smallest model"),
            "testing a transcription with the smallest model"
        );
    }

    #[test]
    fn normalize_response_preserves_newlines_after_sentence_end() {
        assert_eq!(normalize_response("hello.\nworld"), "hello.\nworld");
        assert_eq!(normalize_response("hello!\nworld"), "hello!\nworld");
        assert_eq!(normalize_response("hello?\nworld"), "hello?\nworld");
    }

    #[test]
    fn normalize_response_preserves_newlines_before_capital() {
        assert_eq!(normalize_response("hello\nWorld"), "hello\nWorld");
    }

    #[test]
    fn normalize_response_preserves_single_newlines() {
        assert_eq!(normalize_response("hello\nworld"), "hello world"); // Changed: single newlines now collapse
    }

    #[test]
    fn normalize_response_preserves_double_newlines() {
        assert_eq!(normalize_response("hello\n\nworld"), "hello\n\nworld");
    }

    #[test]
    fn normalize_response_trims_whitespace() {
        assert_eq!(normalize_response("  hello  "), "hello");
        assert_eq!(normalize_response("\n\nhello\n\n"), "hello");
    }

    #[test]
    fn normalize_response_handles_empty_string() {
        assert_eq!(normalize_response(""), "");
    }

    #[test]
    fn normalize_response_handles_only_newlines() {
        assert_eq!(normalize_response("\n\n\n"), "");
    }
}
