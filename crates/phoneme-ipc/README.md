# 📡 phoneme-ipc

This crate defines the wire protocol schema and the named-pipe transport used by Phoneme.

## 🗂️ Responsibilities

- **Schema**: Defines the `Request`, `Response`, and `DaemonEvent` enums using `serde`. By sharing this crate across the CLI, GUI, and Daemon, the API can never drift out of sync.
- **Transport**: Provides the `NamedPipeTransport` and `JsonLineCodec` to facilitate real-time, bidirectional communication over Windows Named Pipes (`\\.\pipe\phoneme-daemon`).
