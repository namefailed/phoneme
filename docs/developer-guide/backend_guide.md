# Backend Developer Guide — the Rust workspace

Phoneme's backend is a headless, local-first daemon plus a companion CLI,
written in **Rust** with **Tokio** for async orchestration and **SQLx** for
persistence. This guide is the map: which crate owns what, and the patterns the
daemon leans on. For the runtime *journey* read [architecture.md](architecture.md);
for subsystem mechanics read [internals.md](internals.md).

Every crate and module carries its own rustdoc. The crate-level docs are system
maps in their own right — read them first:

- [`phoneme-core/src/lib.rs`](../../crates/phoneme-core/src/lib.rs) — modules
  grouped by pipeline stage.
- [`phoneme-audio/src/lib.rs`](../../crates/phoneme-audio/src/lib.rs) — the
  capture/encode path.
- [`phoneme-ipc/src/lib.rs`](../../crates/phoneme-ipc/src/lib.rs) — the wire
  contract and compatibility rules.

Generate the HTML locally with `cargo doc --workspace --no-deps --open`.

---

## 1. Workspace map

The repository is a Cargo workspace of three library crates and three binaries
(plus the frontend, which is a separate Vite/TypeScript app — see
[frontend_guide.md](frontend_guide.md)).

### Library crates (`crates/`)

| Crate | Owns | Notable modules |
| :--- | :--- | :--- |
| [`phoneme-core`](../../crates/phoneme-core/src/lib.rs) | All domain logic & the data layer, knows nothing about IPC/windows/hotkeys | [`config`](../../crates/phoneme-core/src/config.rs), [`transcription`](../../crates/phoneme-core/src/transcription.rs), [`llm`](../../crates/phoneme-core/src/llm.rs), [`diarization`](../../crates/phoneme-core/src/diarization.rs), [`catalog`](../../crates/phoneme-core/src/catalog.rs), [`chunk`](../../crates/phoneme-core/src/chunk.rs)/[`embed`](../../crates/phoneme-core/src/embed.rs)/[`fusion`](../../crates/phoneme-core/src/fusion.rs), [`hook`](../../crates/phoneme-core/src/hook.rs)/[`webhook`](../../crates/phoneme-core/src/webhook.rs), [`doctor`](../../crates/phoneme-core/src/doctor.rs), [`queue`](../../crates/phoneme-core/src/queue.rs), [`types`](../../crates/phoneme-core/src/types.rs) |
| [`phoneme-audio`](../../crates/phoneme-audio/src/lib.rs) | Capture & encoding to canonical 16 kHz mono i16 | [`recorder`](../../crates/phoneme-audio/src/recorder.rs), [`source`](../../crates/phoneme-audio/src/source.rs), [`convert`](../../crates/phoneme-audio/src/convert.rs), [`silence`](../../crates/phoneme-audio/src/silence.rs), [`preroll`](../../crates/phoneme-audio/src/preroll.rs), [`decode`](../../crates/phoneme-audio/src/decode.rs), [`meeting_align`](../../crates/phoneme-audio/src/meeting_align.rs) |
| [`phoneme-ipc`](../../crates/phoneme-ipc/src/lib.rs) | The wire contract between daemon and clients | [`schema`](../../crates/phoneme-ipc/src/schema.rs) (the protocol reference), [`codec`](../../crates/phoneme-ipc/src/codec.rs), [`named_pipe`](../../crates/phoneme-ipc/src/named_pipe.rs), [`transport`](../../crates/phoneme-ipc/src/transport.rs) |

### Binaries (`bin/`) and the tray (`src-tauri/`)

| Binary | Owns | Notable modules |
| :--- | :--- | :--- |
| [`phoneme-daemon`](../../bin/phoneme-daemon/src/main.rs) | The brain — IPC server, recorder glue, inbox worker, pipeline, supervisors, event bus | [`main`](../../bin/phoneme-daemon/src/main.rs), [`app_state`](../../bin/phoneme-daemon/src/app_state.rs), [`recorder`](../../bin/phoneme-daemon/src/recorder.rs), [`queue_worker`](../../bin/phoneme-daemon/src/queue_worker.rs), [`pipeline`](../../bin/phoneme-daemon/src/pipeline.rs), [`in_place`](../../bin/phoneme-daemon/src/in_place.rs), [`whisper_supervisor`](../../bin/phoneme-daemon/src/whisper_supervisor.rs), [`ollama_launcher`](../../bin/phoneme-daemon/src/ollama_launcher.rs), [`event_bus`](../../bin/phoneme-daemon/src/event_bus.rs), [`shutdown`](../../bin/phoneme-daemon/src/shutdown.rs), [`reconcile`](../../bin/phoneme-daemon/src/reconcile.rs), [`retention`](../../bin/phoneme-daemon/src/retention.rs) |
| [`phoneme`](../../bin/phoneme/src/main.rs) | The CLI — one module per subcommand, translating to IPC requests | [`args`](../../bin/phoneme/src/args.rs) (clap), [`client`](../../bin/phoneme/src/client.rs) (spawn vs observe), [`commands/`](../../bin/phoneme/src/commands/mod.rs) |
| [`src-tauri`](../../src-tauri/src/lib.rs) | The Tauri 2 tray shell — spawn/bridge the daemon, forward commands, re-emit events | [`lib`](../../src-tauri/src/lib.rs), [`auto_spawn`](../../src-tauri/src/auto_spawn.rs), [`bridge`](../../src-tauri/src/bridge.rs), [`commands`](../../src-tauri/src/commands/mod.rs), [`events`](../../src-tauri/src/events.rs), [`tray`](../../src-tauri/src/tray.rs), [`overlay`](../../src-tauri/src/overlay.rs), [`wizard`](../../src-tauri/src/wizard.rs)/[`checksums`](../../src-tauri/src/checksums.rs) |

**The dependency arrow points one way.** `phoneme-core` is the shared substrate;
the daemon, CLI, and tray all depend on it (and on `phoneme-ipc`), never the
reverse. `phoneme-core` deliberately has no knowledge of pipes, windows, or
hotkeys — it transcribes, post-processes, stores, and answers questions, and the
daemon wires those pieces into a running pipeline.

---

## 2. Tokio async architecture & actors

The daemon is a multi-threaded async process; tasks cooperate over channels and
synchronization primitives rather than shared mutable globals. The task topology
diagram and channel table are in
[internals.md](internals.md#async-task-topology).

### The actor pattern

Exclusive hardware (microphone, loopback) is owned by an **actor**
([`recorder.rs`](../../crates/phoneme-audio/src/recorder.rs)): the `Recorder`
exposes a `Sender<RecorderCommand>` rather than its buffers, and a long-running
loop owns the device state and processes commands (`Stop`, `Cancel`, `Pause`,
`Resume`, `Snapshot`) one at a time, replying over a `oneshot`. The daemon-side
glue ([`bin/phoneme-daemon/src/recorder.rs`](../../bin/phoneme-daemon/src/recorder.rs))
ties that lifecycle to the catalog, the inbox queue, and the event bus, and owns
the "at most one capture", toggle-atomicity, and no-slow-await-under-a-lock
invariants.

### Semaphore resource limits

Transcription is heavy, so the daemon gates the bundled whisper-server with a
shared `whisper_sem` permit in [`AppState`](../../bin/phoneme-daemon/src/app_state.rs):
a final transcription holds the permit for its whole STT call, and the live
preview only ticks when the permit is free — so a preview can never starve a real
transcription, and a one-job model-override swap happens *under* the permit so
nothing else talks to the server mid-restart.

### Shared state

Everything that outlives a request hangs off [`AppState`](../../bin/phoneme-daemon/src/app_state.rs):
the hot-swappable config, catalog, inbox queue, event bus, recorder, shared
HTTP-backed provider clients, and the shutdown coordinator. Cloning `AppState`
clones `Arc`s, so every task sees the same components. It also holds the
coordination cells — the one-job `WhisperModelOverride`, the `WhisperEffectivePorts`
published after port fallback, the `processing` slot (in-flight id + cancel
token), and the kill-on-close job object.

---

## 3. SQLite, WAL & FTS5 (`phoneme-core`)

A single SQLite file (`catalog.db`) persists recordings, transcripts, segments,
tags, and embedding vectors. The connection options (WAL, `synchronous=NORMAL`,
bounded autocheckpoint, idle checkpointing), the three-transcript-layer design,
the FTS5 sync triggers and query sanitizing, and the in-memory embedding cache
are all detailed in
[internals.md](internals.md#sqlite-catalog--search-internals). The module is
[`catalog.rs`](../../crates/phoneme-core/src/catalog.rs).

---

## 4. Child-process supervision

Local transcription runs via a bundled C++ whisper-server. The
[`whisper_supervisor`](../../bin/phoneme-daemon/src/whisper_supervisor.rs) keeps
it (and a second, thread-capped preview server when configured) alive:

- **Respawn loop** — spawn the binary, then watch four wake sources at once:
  child exit (respawn with 2 s → 60 s backoff, reset after a healthy minute), a
  spec-change poll (model/port/mode differs from what the child was spawned
  with), an explicit `whisper_restart` notify (the Doctor "Fix" — the only path
  that heals a *hung* server), and shutdown. Even the crash backoff is
  cancellable by restart/shutdown so a Doctor fix is never lost.
- **Effective ports** — the configured port is a preference; a pre-flight probe
  routes around a foreign squatter to a free OS-assigned port (excluding the
  sibling server's ports), publishes the choice to `AppState::whisper_ports`
  *before* the spawn, and clears it when down.
- **One-job model overrides** — the spawn uses `effective_model_path`
  (override-if-set, else config) and the spec-change check compares the same
  effective value, so a model-override re-transcription is exactly one
  restart-to-override plus one restore — never a config-mutation thrash.
- **Job membership & sweeps** — every child joins the kill-on-close job;
  `sweep_stray_servers` also kills the whisper-server processes it can identify
  by their transcription command line (every server Phoneme spawns carries the
  `/v1/audio/transcriptions` marker) to free squatted ports and hung orphans
  before a respawn — an unrelated `whisper.cpp` launched for something else is
  left alone.
- **No pipe wedging** — the child's stdout/stderr are discarded; a piped-but-
  undrained child blocks once the OS buffer fills and silently hangs
  transcription.

A Phoneme-launched Ollama is supervised separately by
[`ollama_launcher`](../../bin/phoneme-daemon/src/ollama_launcher.rs) under the
ownership ledger described in
[architecture.md](architecture.md#2-process-lifecycle--ownership).

---

## 5. Dual-track meeting alignment (`phoneme-audio`)

Meeting mode records a dense mic track and a sparse WASAPI loopback track, then
reconstructs both onto one wall-clock timeline. The sparse-detection and
silence-padding math lives in
[internals.md](internals.md#dual-track-alignment-math); the module is
[`meeting_align.rs`](../../crates/phoneme-audio/src/meeting_align.rs).

---

## 6. Where to make common changes

| To add… | Touch | Then |
| :--- | :--- | :--- |
| A new IPC command | [`schema.rs`](../../crates/phoneme-ipc/src/schema.rs) (variant + doc), [`ipc_handler.rs`](../../bin/phoneme-daemon/src/ipc_handler.rs) (handler) | a CLI command and/or a tray [`commands.rs`](../../src-tauri/src/commands/mod.rs) forward |
| A new transcription/LLM provider | [`transcription.rs`](../../crates/phoneme-core/src/transcription.rs) / [`llm.rs`](../../crates/phoneme-core/src/llm.rs) | config in [`config.rs`](../../crates/phoneme-core/src/config.rs), the picker in [frontend_guide.md](frontend_guide.md) |
| A new pipeline stage | [`pipeline.rs`](../../bin/phoneme-daemon/src/pipeline.rs) | a `PipelineStage`/`DaemonEvent` if it needs UI progress |
| A new config key | [`config.rs`](../../crates/phoneme-core/src/config.rs) (`#[serde(default)]`!) | [config_reference.md](config_reference.md) + the masking list if it's a secret |

See [how_to_extend.md](how_to_extend.md) for step-by-step recipes and
[ipc_integration.md](ipc_integration.md) for the wire format. House rules,
target-test isolation, and the local check commands are in
[testing_and_ci.md](testing_and_ci.md) and [CONTRIBUTING.md](../../CONTRIBUTING.md).
