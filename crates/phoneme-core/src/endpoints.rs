//! Canonical default base/chat URLs for every cloud provider.
//!
//! These literals were duplicated between the live provider paths
//! (`transcription.rs`, `llm.rs`) and the diagnostic paths (`doctor.rs`), so a
//! provider's default endpoint had to be edited in two places and could silently
//! drift — the doctor probing one URL while a recording hit another. This module
//! is the single source of truth both sides import.
//!
//! STT consts are *base* URLs (no trailing path): the transcription providers
//! append their own route. LLM consts are the *full* chat/generate URLs the
//! provider POSTs to. Groq's STT base deliberately keeps the `/openai` suffix —
//! its API is OpenAI-compatible under that path.

// ── Speech-to-text base URLs ─────────────────────────────────────────────────

/// OpenAI cloud Whisper base URL.
pub const OPENAI_STT_BASE: &str = "https://api.openai.com";
/// Groq cloud Whisper base URL (OpenAI-compatible under the `/openai` path).
pub const GROQ_STT_BASE: &str = "https://api.groq.com/openai";
/// Deepgram speech-to-text base URL.
pub const DEEPGRAM_STT_BASE: &str = "https://api.deepgram.com";
/// AssemblyAI speech-to-text base URL.
pub const ASSEMBLYAI_STT_BASE: &str = "https://api.assemblyai.com";
/// ElevenLabs Scribe speech-to-text base URL.
pub const ELEVENLABS_STT_BASE: &str = "https://api.elevenlabs.io";

// ── LLM chat/generate URLs ───────────────────────────────────────────────────

/// Local Ollama generate endpoint.
pub const OLLAMA_LLM_URL: &str = "http://127.0.0.1:11434/api/generate";
/// OpenAI chat-completions endpoint.
pub const OPENAI_LLM_URL: &str = "https://api.openai.com/v1/chat/completions";
/// Groq chat-completions endpoint (OpenAI-compatible).
pub const GROQ_LLM_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
/// Anthropic messages endpoint.
pub const ANTHROPIC_LLM_URL: &str = "https://api.anthropic.com/v1/messages";
