# phoneme-daemon

The headless brain of [Phoneme](../../README.md). Owns audio capture, the
inbox queue, the SQLite catalog, and (in bundled modes) the whisper-server
process. Exposes all operations over a Windows named-pipe IPC surface.

Clients: `phoneme` CLI, the Tauri GUI shell (`phoneme-tray`), and external scripts using the `phoneme-ipc` crate.

## Architecture & Modules

| Module | Responsibility |
|---|---|
| `app_state` | `AppState` and `ResolvedPaths` — central component holder |
| `event_bus` | `tokio::sync::broadcast` channel for `DaemonEvent`s |
| `ipc_server` | Accept loop on `\\.\pipe\<name>` |
| `ipc_handler` | `Request` → `Response` routing + `SubscribeEvents` streaming |
| `whisper_supervisor` | Spawns/monitors `whisper-server.exe` in bundled modes |
| `logging` | tracing-subscriber: stderr (`--foreground`) / JSON file (default) |
| `pipeline` | Per-payload pipeline: transcribe → hook → done |
| `queue_worker` | Drains `inbox/pending/` serially; exponential backoff on Whisper outage |
| `reconcile` | Startup recovery (orphan inbox files, stale catalog rows) |
| `recorder` | Daemon-side recorder wrapper; owns the single active recording |
| `shutdown` | Ctrl+C handler + `watch::Sender<bool>` shutdown coordinator |

## Single instance

Enforced exclusively by `NamedPipeListener::bind(name).first_pipe_instance(true)`.
No PID lockfile (Windows recycles PIDs aggressively, which makes lockfiles
unreliable). A second daemon attempting to bind the same pipe name fails
fast with the friendly message:

```
another phoneme-daemon is already running. Stop it with `phoneme daemon --stop`.
```

## Environment overrides

Both honored at startup and used by the integration test harness:

- `PHONEME_CONFIG=<path>` — load config from this TOML file instead of the
  user's `%APPDATA%\phoneme\config.toml` (Windows).
- `PHONEME_DATA_LOCAL=<dir>` — redirect inbox / catalog / log files away
  from `%LOCALAPPDATA%\phoneme\` so tests don't clobber a real install.

## Running

```bash
# Foreground, pretty logs to stderr.
cargo run -p phoneme-daemon -- --foreground

# Background (default), JSON logs to %LOCALAPPDATA%\phoneme\logs\daemon.log
cargo run -p phoneme-daemon
```

## Testing

```bash
cargo test -p phoneme-daemon -- --test-threads=1
```

Integration tests use the `DaemonHarness` in `tests/common/mod.rs`, which
spins up a temp directory, a wiremock-backed stub whisper-server, and the
real daemon binary. `--test-threads=1` keeps named-pipe bind races out of
the picture during early development; revisit once we add pipe-name
randomization to the harness.

Recording-flow integration tests (basic_flow, whisper_unreachable, hook_timeout,
crash_recovery, concurrent_record, replay, event_stream, rebuild_catalog)
rely on synthetic-audio plumbing via `--test-mode` flags to assert deterministic transcriptions without requiring physical microphone access.
