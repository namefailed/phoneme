# phoneme-ipc

IPC schema and transport for [Phoneme](../../README.md).

## Modules

| Module | Responsibility |
|---|---|
| `schema` | `Request`, `Response`, `DaemonEvent`, `IpcError`, `IpcErrorKind` — the wire format |
| `codec` | Newline-delimited JSON framing for tokio_util |
| `transport` | `Transport` trait — transport-agnostic surface for clients |
| `named_pipe` | Windows named-pipe implementation of `Transport` |
| `error` | `IpcTransportError` for transport-layer failures (distinct from response errors) |

## Wire format

All messages are JSON, one per line. The pipe name is `\\.\pipe\phoneme-daemon`
by default (configurable in the daemon's `[daemon] pipe_name`).

### Request flow

```
Client                  Daemon
   |                       |
   |---{type:record_start,mode:{hold:null}}\n--->|
   |                       |
   |<--{status:ok,value:null}\n------------------|
```

### Subscription flow

```
Client                  Daemon
   |                       |
   |---{type:subscribe_events}\n---------------->|
   |                       |
   |<--{event:recording_started,id:...}\n--------|
   |<--{event:transcription_done,id:...}\n-------|
   |<--{event:hook_done,id:...,exit_code:0}\n----|
   |                       |
   X(client closes)        |
```

## Transport-agnostic design

The `Transport` trait abstracts the wire. A future v2.0 may add `HttpTransport`
for mobile clients without touching `schema.rs`:

```rust
#[async_trait]
pub trait Transport: Send + Sync {
    async fn request(&mut self, req: Request) -> TransportResult<Response>;
    async fn subscribe(&mut self) -> TransportResult<BoxStream<'static, TransportResult<DaemonEvent>>>;
}
```

## Running the tests

```bash
cargo test -p phoneme-ipc -- --test-threads=1
```

`--test-threads=1` is used for the round-trip tests because they bind real
named pipes; running them in parallel works but adds flake risk during early
development.
