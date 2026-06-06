# ⚙️ Phoneme Internals

A developer's-eye map of how Phoneme works under the hood. Read
[architecture.md](architecture.md) first for the high-level triad; this document
goes a layer deeper into the async task topology, the audio path, the SQLite
catalog, the IPC wire protocol, and the filesystem queue.

> Audience: contributors. If you just want to *use* Phoneme, see the
> [README](../README.md).

## 🗂️ Workspace layout

```
crates/
  phoneme-core    Shared models, config, catalog, queue, transcription, llm, hook, webhook
  phoneme-ipc     Wire protocol (schema) + transport (named pipe, NDJSON codec)
  phoneme-audio   Capture (cpal), resample/convert, silence, pre-roll, decode, WAV
bin/
  phoneme-daemon  Headless engine: IPC server, queue worker, recorder, pipeline
  phoneme         CLI client
src-tauri/        Tauri GUI backend (crate `phoneme-tray`)
frontend/         Vanilla-TS GUI (Vite)
hooks/            Reference hook scripts shipped to users
```

Dependency direction: `phoneme-audio` and the binaries depend on `phoneme-core`;
nothing depends on the binaries. `phoneme-ipc` is shared by all three binaries so
the wire format can never drift between client and daemon (adding a `Request`
variant is a compile error until every match arm handles it).

## 🕸️ Async task topology (daemon)

The daemon is a Tokio application. The long-lived tasks and the channels between
them:

| Task / module | Role | Talks via |
|---|---|---|
| `ipc_server` | Accept loop on `\\.\pipe\<name>` | spawns a handler task per connection |
| `ipc_handler` | Routes one `Request` → `Response`; streams events for `SubscribeEvents` | reads `event_bus` |
| `event_bus` | Fan-out of `DaemonEvent`s to all subscribers | `tokio::sync::broadcast` |
| `queue_worker` | Drains `inbox/pending/` serially; runs the pipeline per item | reads the filesystem queue |
| `pipeline` | transcribe → (LLM) → hooks → done, per payload | calls providers, writes catalog |
| `recorder` | Owns the active recording / meeting / pre-roll / streaming preview | `mpsc` commands, `oneshot` replies |
| `whisper_supervisor` | Spawns/monitors `whisper-server.exe` in bundled modes | child process |
| `shutdown` | Ctrl+C handler → coordinated shutdown | `tokio::sync::watch<bool>` |

Channel cheat-sheet (in `phoneme-audio`/daemon `recorder`):

- **`mpsc`** — the recorder's command channel (`Stop`/`Cancel`/`Pause`/`Resume`/`Snapshot`).
- **`oneshot`** — `Snapshot` reply (a clone of the in-progress samples) and the
  recorder's `on_done` signal.
- **`broadcast`** — `DaemonEvent`s, so the CLI `watch` and the GUI both see the
  same stream live (this is why driving the daemon from the CLI updates the GUI
  in real time, and vice-versa).
- **`watch`** — shutdown flag observed by all loops.

## ⏱️ Lifecycle of a recording

1. **Trigger** — `RecordStart`/`RecordToggle` (or `StartMeeting` for dual-track)
   arrives over IPC.
2. **Capture** — the recorder opens a `cpal` stream and pulls audio. If pre-roll
   is enabled, the buffered idle audio is prepended so the first syllable isn't
   clipped. A catalog row is inserted at `status = recording`.
3. **Finalize** — on `RecordStop` the capture task drains its tail and writes a
   `.wav`; a payload file is dropped into `inbox/pending/`.
4. **Queue** — `queue_worker` claims the payload and invokes the `pipeline`.
5. **Transcribe** — the configured `TranscriptionProvider` runs; the raw output
   is preserved as `original_transcript`.
6. **Post-process** — optional LLM cleanup; the cleaned text becomes the live
   `transcript` while the raw stays in `original_transcript`.
7. **Hooks** — unless `hook.run_on_transcribe` is off, the always-on `commands`
   run, then any matching `keyword_rules`, then the webhook fires.
8. **Done** — status → `done`, the payload moves to `inbox/done/`.

Imported files (`ImportRecording`) skip 1–3: the file is decoded to canonical
form, copied into the audio dir, and enters at step 4.

## 🔊 Audio path (`phoneme-audio`)

The canonical format is **16 kHz, mono, signed 16-bit PCM**. Everything converges
on it.

- **`source`** — the `Source` trait. `CpalSource` opens a microphone *or*, on
  Windows, the default output device in **WASAPI loopback** mode (system audio).
  `SyntheticSource` feeds hand-crafted PCM in tests so the pipeline runs with no
  hardware.
- **`convert`** — `f32 ↔ i16`, stereo→mono downmix, and resampling to 16 kHz via
  `rubato`. Live capture streams in fixed chunks; import resamples the decoded
  buffer.
- **`silence`** — `SilenceDetector` (RMS over a rolling window) drives
  auto-stop-on-silence; it's `reset()` on resume.
- **`preroll`** — `PreRollBuffer`, a ring buffer of the last *N* ms of idle
  microphone audio, prepended to a recording so the opening word survives. Those
  prepended samples are *not* fed to the silence detector (they're historical).
- **`recorder`** — the state machine: start/stop/cancel/pause/resume, the
  `Snapshot` command (clone the in-progress buffer for the streaming preview
  without disturbing capture), and `start_with_prepend` for pre-roll.
- **`decode`** — imports `.mp3`/`.m4a`/`.wav` via `symphonia`, bounded by a
  max-duration cap so a crafted file can't OOM the daemon.
- **`wav`** — final WAV encode/decode via `hound`.

## 🗄️ SQLite catalog (`phoneme-core::catalog`)

A single SQLite database, accessed with `sqlx` and versioned migrations
(`phoneme-core/migrations`). Opened in **WAL** mode with `synchronous=NORMAL`,
`wal_autocheckpoint`, and a `journal_size_limit` cap; the daemon also checkpoints
on idle to bound WAL growth.

- **`recordings`** — the central table: `id`, `started_at`, `duration_ms`,
  `audio_path`, `transcript`, `original_transcript`, `model`, `status`, hook
  result columns, `notes`, and the meeting-link columns `meeting_id` + `track`.
- **FTS5** — a full-text index mirrors the transcript so `list` search is fast.
  It's kept in sync with the `recordings` table via triggers, so an insert /
  update / delete automatically updates the index. User search text is sanitised
  into a robust `term* AND term*` prefix query before it ever reaches SQLite
  (`sanitize_fts5_query`), so odd input can't crash the matcher.
- **`tags`** / **`recording_tags`** — colour-coded tags, many-to-many.

Audio lives on disk under a date-foldered directory, **not** in the DB — the
SQLite file stays small and copyable.

## 📡 IPC (`phoneme-ipc`)

- **Transport** — a Windows named pipe (`\\.\pipe\phoneme-daemon`), framed as
  **newline-delimited JSON** (`JsonLineCodec`): one JSON value per line.
- **`Request`** — client → daemon, serde-tagged on `"type"` (snake_case):
  `record_start`, `start_meeting`, `list_recordings`, `get_recording`,
  `list_meeting`, `retranscribe_recording`, `import_recording`, `update_notes`,
  `reload_config`, `shutdown`, … plus tag ops.
- **`Response`** — daemon → client, tagged on `"status"`: `Ok(value)` or
  `Err(IpcError)`. `IpcError` carries a machine-readable `kind`
  (`already_recording`, `not_found`, `whisper_unreachable`, …) + a human message.
- **`DaemonEvent`** — daemon → all subscribers, tagged on `"event"`
  (`recording_started`, `transcription_partial`, `queue_depth_changed`,
  `notes_updated`, …). Clients send `subscribe_events` and then receive the
  one-way stream.
- **`Transport` trait** — abstracts the wire so a future `HttpTransport` (v2.0
  mobile/REST) can be added without touching `schema.rs`.

## 📥 Inbox queue (`phoneme-core::queue`)

A filesystem-backed work queue under the data dir:

```
inbox/pending/    waiting to be processed
inbox/processing/ claimed by the worker
inbox/done/       completed
inbox/failed/     errored (with reason)
```

State transitions are **atomic renames** between these directories. The worker
claims the head item by renaming it into `processing/` *before* parsing it, so a
single corrupt file can't wedge the queue — a filename that isn't a canonical
`RecordingId` is **quarantined** rather than parsed (parsing untrusted filenames
with `from_str_unchecked` would panic the daemon; the queue deliberately uses the
fallible `RecordingId::parse` + quarantine instead). `reconcile` recovers orphans
left by a crash on startup.

## 🎨 Frontend (`frontend/`)

Deliberately framework-less Vanilla TypeScript (Vite). Components are classes
that build `innerHTML`.

- **`state/Store.ts`** — a tiny reactive store; components subscribe and re-render.
- **`router.ts`** — switches between the main views.
- **`services/ipc.ts`** — the typed boundary to the Tauri commands.
- **`services/events.ts`** — subscribes to `daemon-event` and dispatches to
  handlers.

> **Security invariant:** any dynamic / user-influenced string interpolated into
> `innerHTML` **must** go through `escapeHtml` / `escapeAttr` (`utils/format.ts`).
> Transcripts, notes, file paths, tag names, search terms, and session ids are
> all attacker-influenced. `highlightMatch` escapes in every branch.

## 🧪 Testing without hardware

- **`SyntheticSource`** feeds canned PCM, so recorder/pipeline tests run with no
  microphone.
- **`DaemonHarness`** (`bin/phoneme-daemon/tests/common`) spins up a temp data
  dir, a `wiremock` stub whisper-server (routed via `WhisperMode::External`), and
  the real daemon binary over a unique pipe name. `start_with(|cfg| …)` lets a
  test tweak the config (hook commands, `run_on_transcribe`, keyword rules)
  before launch.
- Device-dependent `cpal` tests early-return when no input device is present, so
  CI stays green without audio hardware. (A mock cpal backend for true
  end-to-end CI capture is a known gap — see the roadmap.)

Run it all: `cargo test --workspace` (Rust) and `npm test --prefix frontend`
(vitest, Node 20).
