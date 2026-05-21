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
| `types` | `Recording`, `RecordingStatus`, `RecordMode`, `HookPayload`, `ListFilter` |
| `catalog` | SQLite-backed recordings catalog (WAL mode + FTS5 search) |
| `queue` | Filesystem-backed inbox queue with atomic state transitions |
| `transcription` | HTTP client for `/v1/audio/transcriptions` (OpenAI-compatible) |
| `hook` | Subprocess runner for user hook scripts (stdin JSON + timeout) |

## Public API stability

The crate is `1.0.0-dev` and does not yet commit to a stable API. Once Phoneme
ships v1.0 the public surface here will be stabilised.

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
