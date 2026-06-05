# phoneme-core

Shared library for the [Phoneme](../../README.md) voice notes app.

This crate is platform-agnostic and provides the building blocks consumed by
`phoneme-daemon`, the `phoneme` CLI, and the Tauri tray app.

## Modules

| Module | Responsibility |
|---|---|
| `config` | TOML config loading, validation, and `~`/`%VAR%` expansion |
| `error` | Single `Error` enum mirroring the IPC `ErrorKind` taxonomy |
| `id` | `RecordingId` (sortable `YYYYMMDDTHHmmssNNN` string, per-process monotonic counter) |
| `types` | `Recording` (incl. `notes` and the `session_id` / `track` meeting links), `RecordingStatus`, `RecordMode`, `MeetingTrack`, `HookPayload`, `ListFilter` |
| `catalog` | SQLite-backed recordings catalog (WAL mode + FTS5 search) |
| `profiles` | Named config-profile storage with a path-traversal-safe name whitelist |
| `tags` | Colour-coded tag model and validation |
| `queue` | Filesystem-backed inbox queue with atomic state transitions |
| `transcription` | `TranscriptionProvider` abstraction: local whisper.cpp plus OpenAI, Deepgram, AssemblyAI, Groq, ElevenLabs, and any OpenAI-compatible endpoint |
| `llm` | LLM post-processing provider abstraction (OpenAI-compatible, Groq, Anthropic) |
| `hook` | Subprocess runner for user hook scripts (stdin JSON + timeout) |
| `webhook` | Optional HTTP POST of the hook payload to a configured URL |

## Public API stability

This crate is internal to the Phoneme workspace and does not commit to a stable
public API; its surface may change between releases as the binaries evolve.

## Hook contract

When invoked, hook subprocesses receive a JSON object on stdin matching the
`HookPayload` struct. `metadata.hook_version` is a stability commitment: while
it stays `1`, surrounding fields will not be renamed or removed.

See the top-level design doc for full details.

## Running the tests

```bash
cargo test -p phoneme-core --workspace
```

The `transcription` integration tests spin up an in-process [wiremock] HTTP
server. The `hook` tests create small platform-appropriate scripts in a
`tempfile::TempDir`. No external services are required.

[wiremock]: https://crates.io/crates/wiremock
