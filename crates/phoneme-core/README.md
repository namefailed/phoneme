# 🧠 phoneme-core

This crate contains the shared business logic, database models, configuration schema, and the background queue worker for Phoneme.

## 🗂️ Responsibilities

- **Catalog** (`catalog`): SQLite interactions and migrations via `sqlx`, including
  FTS5 keyword search and `hybrid_search` (semantic ⊕ keyword).
- **Config** (`config`): user settings parsed from TOML (`Config`, provider configs,
  `SemanticSearchConfig`).
- **Queue** (`queue`): filesystem-backed job queue for asynchronous processing.
- **Transcription** (`transcription`): the `TranscriptionProvider` trait + local
  whisper.cpp and cloud (OpenAI/Groq/Deepgram/AssemblyAI/ElevenLabs/custom) backends.
- **Diarization** (`diarization`): speaker-turn assignment (offline speakrs ONNX, or
  cloud via the transcription providers).
- **Semantic search** (`chunk` + `embed` + `fusion`): sentence-aware chunking, ONNX
  embeddings, and Reciprocal Rank Fusion + cosine calibration.
- **Hooks / Webhooks** (`hook`, `webhook`): execution logic for local scripts and
  HTTP POST targets.
- **LLM** (`llm`): Smart Cleanup / summary API bindings for Ollama, OpenAI-compatible,
  Groq, and Anthropic.
- **Doctor** (`doctor`): diagnostics and catalog rebuild.
