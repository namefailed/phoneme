# 🧠 phoneme-core

This crate contains the shared business logic, database models, configuration schema, and the background queue worker for Phoneme.

## 🗂️ Responsibilities

- **Catalog**: SQLite database interactions and migrations via `sqlx`.
- **Config**: User settings parsed from TOML (`Config`, `TranscriptionProvider`).
- **Queue**: Filesystem-backed job queue for asynchronous processing.
- **Hooks**: Execution logic for Webhooks, Python, and local scripts.
- **LLM**: Smart Cleanup API bindings for Ollama, OpenAI, and Anthropic.
