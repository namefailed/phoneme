# 📡 phoneme-ipc

This crate defines the wire protocol schema and the named-pipe transport used by Phoneme.

## 🗂️ Responsibilities

- **Schema**: Defines the `Request`, `Response`, and `DaemonEvent` enums using `serde`. By sharing this crate across the CLI, GUI, and Daemon, the API can never drift out of sync.
- **Transport**: Provides the `NamedPipeTransport` and `JsonLineCodec` to facilitate real-time, bidirectional communication over Windows Named Pipes (`\\.\pipe\phoneme-daemon`).

## 🛡️ Robustness

- **Lenient server decode (`ServerRequest`)**: the server decodes each line as
  `ServerRequest`, not a bare `Request`. A well-formed JSON line that isn't a
  recognized variant (e.g. a newer client during a rolling rebuild) decodes to
  `ServerRequest::Unknown` so the daemon can answer with an error `Response` and
  **keep the connection alive** — one unknown request never tears down the pipe.
- **Owner-only pipe ACL** (`D:P(A;;GA;;;OW)(A;;GA;;;SY)`) on every instance, and an
  **8 MiB NDJSON frame cap** (`codec`) so an oversized/unterminated frame errors
  instead of growing the buffer unbounded.
