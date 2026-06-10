# ⚙️ Phoneme Internals

A developer's-eye map of how Phoneme works under the hood. Read
[architecture.md](architecture.md) first for the high-level triad; this document
goes a layer deeper into the async task topology, the audio path, the SQLite
catalog, the IPC wire protocol, and the filesystem queue.

> Audience: contributors. If you just want to *use* Phoneme, see the
> [README](../../README.md).

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
   `transcript`, is also preserved as `clean_transcript`, while the raw stays in
   `original_transcript`.
7. **Hooks** — unless `hook.run_on_transcribe` is off, the always-on `commands`
   run, then any matching `keyword_rules`, then the webhook fires.
8. **Summary** — if `summary.auto` is on, an LLM summary is generated as the final
   step and stored in `summary` / `summary_model`.
9. **Done** — status → `done`, the payload moves to `inbox/done/`.

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

- **`recordings`** — the central table. Beyond `id`, `started_at`,
  `duration_ms`, `audio_path`, `model`, `status`, and the hook-result columns, it
  carries:
  - **Three transcript layers:** `original_transcript` (raw machine output,
    pre-cleanup), `clean_transcript` (pipeline output — transcribed + cleaned —
    pre-hand-edit), and `transcript` (the current, possibly user-edited text).
  - **Summary:** `summary` and `summary_model` (the AI summary and the model that
    produced it; null until generated).
  - **Meeting link:** `meeting_id`, `meeting_name`, `track` (`mic` / `system`).
    Standalone recordings have a null `meeting_id`.
  - Plus `notes`, `cleanup_model`, `in_place`, and `diarized`.
- **FTS5** — `recordings_fts` mirrors the transcript so `list` search is fast.
  It's kept in sync with the `recordings` table via triggers, so an insert /
  update / delete automatically updates the index. User search text is sanitised
  into a robust `term* AND term*` prefix query before it ever reaches SQLite
  (`sanitize_fts5_query`), so odd input can't crash the matcher.
- **`tags`** / **`recording_tags`** — colour-coded tags, many-to-many.
- **`embedding_chunks`** — the primary semantic-search store: **many** ONNX vectors
  (BLOB) per recording, one per sentence-aware transcript chunk
  (`phoneme-core::chunk`), keyed `(recording_id, chunk_index)`; cascades on delete.
  A recording is scored by its best-matching chunk (max-sim). See *Semantic search*
  below.
- **`embeddings`** — the legacy one-vector-per-recording table, kept only as a
  fallback for rows not yet re-embedded into chunks; the search path prefers
  `embedding_chunks`.

Schema is defined in `crates/phoneme-core/migrations`: the initial schema plus
additive `add_summary`, `add_clean_transcript`, `add_user_edited`, and
`add_embedding_chunks` migrations.

Audio lives on disk under a date-foldered directory, **not** in the DB — the
SQLite file stays small and copyable.

## 🧠 Semantic search (`phoneme-core::chunk` / `embed` / `fusion`)

Semantic search is **chunked and hybrid**, not one-vector-per-recording:

- **`chunk`** — `chunk_transcript` splits a transcript into overlapping,
  sentence-aware windows (~80 words, 1-sentence overlap, capped per recording).
  Short transcripts stay a single chunk. Pure + unit-tested (no model/DB).
- **`embed`** — loads the ONNX model once (`ort`) and embeds each chunk, adapting
  to the configured model via `SemanticSearchConfig` (`max_tokens`, `pooling`
  mean/cls, `token_type_ids`, query/passage prefixes). `pool` and the truncation
  policy are pulled out as pure functions so the math is testable without the
  (unbundled) model.
- **`fusion`** — `reciprocal_rank_fusion` (RRF, `k = 60`) fuses the vector ranking
  with the FTS5 ranking without needing the two score scales to be comparable;
  `calibrate_cosine` maps raw cosine into a 0–100% relevance for the UI.
- **`catalog::hybrid_search`** ties it together: best-chunk cosine ranking ⊕ FTS5
  ranking → RRF → calibrated score. `upsert_chunk_embeddings` replaces a
  recording's chunks transactionally; `list_recordings_without_chunk_embeddings`
  drives the background backfill, and `clear_all_embeddings` + the `ReembedAll`
  IPC re-index the whole library after a model change.

## 📡 IPC (`phoneme-ipc`)

- **Transport** — a Windows named pipe (`\\.\pipe\phoneme-daemon`), framed as
  **newline-delimited JSON** (`JsonLineCodec`): one JSON value per line.
- **`Request`** — client → daemon, serde-tagged on `"type"` (snake_case).
  Recording/meeting control (`record_start`, `record_toggle`, `record_pause`,
  `start_meeting`, `meeting_toggle`, …), catalog queries (`list_recordings`,
  `get_recording`, `list_meeting`), import (`import_recording`), re-processing
  (`retranscribe_recording`, `rerun_cleanup`, `rerun_summary`, `refire_hook`),
  editing (`update_transcript`, `update_notes`, `update_meeting_name`),
  the preserved-transcript fetches (`get_original_transcript`,
  `get_clean_transcript`), tags, semantic search (`semantic_search`,
  `reembed_all`), and lifecycle (`reload_config`, `shutdown`, `subscribe_events`).
  The re-processing requests carry one-time overrides (model/provider/prompt/url/key)
  that are never persisted to config.
- **`ServerRequest` (connection resilience)** — on the **server** side, a line is
  decoded as `ServerRequest`, not a bare `Request`. A well-formed JSON line that
  isn't a recognized variant (e.g. a newer tray sending a request this daemon
  predates during a rolling rebuild) becomes `ServerRequest::Unknown { detail }`
  instead of a codec error. The handler replies with an error `Response` and
  **keeps the connection alive**, so one unknown request can't tear down the pipe
  and break the client's other in-flight commands.
- **`Response`** — daemon → client, tagged on `"status"`: `Ok(value)` or
  `Err(IpcError)`. `IpcError` carries a machine-readable `kind`
  (`already_recording`, `not_found`, `whisper_unreachable`, …) + a human message.
- **`DaemonEvent`** — daemon → all subscribers, tagged on `"event"`
  (`recording_started`, `transcription_partial`, `transcription_done`,
  `summary_updated`, `transcript_updated`, `queue_depth_changed`,
  `notes_updated`, tag events, …). Clients send `subscribe_events` and then
  receive the one-way stream.
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

The frontend is intentionally built for performance and maintainability, leveraging web standards through **Lit** (TypeScript + Vite).

- **`state/Store.ts`** — a custom reactive store; components subscribe to state changes and trigger minimal DOM updates via Lit's decorators.
- **`router.ts`** — simple hash-based router handling navigation between main views.
- **`services/ipc.ts`** — the typed boundary to the Tauri commands.
- **`services/events.ts`** — subscribes to `daemon-event` streams and dispatches to handlers.
- **`overlay.html` + `src/overlay.ts`** — a second, standalone entry (registered as
  an extra Rollup input in `vite.config.ts`) for the system-wide live-preview
  overlay window. It mounts only the caption card — no app shell/router — and is
  driven by the same `daemon-event` stream. The window itself is created at runtime
  by `src-tauri/src/overlay.rs` (frameless, transparent, always-on-top), gated on
  `interface.preview_overlay`. See
  [docs/design/live-preview-overlay.md](../design/live-preview-overlay.md).

> [!IMPORTANT]
> **Security invariant:** Lit's `html` tagged template literals provide automatic contextual escaping, protecting against XSS for most interpolations. However, when using `unsafeHTML` or manually manipulating the DOM, data must still go through `escapeHtml` / `escapeAttr` (`utils/format.ts`). Transcripts, notes, file paths, tag names, search terms, and meeting ids are all attacker-influenced. `highlightMatch` escapes in every branch.
<!-- -->

## 👥 Meeting alignment (`phoneme-audio::meeting_align`)

On `stop_meeting`, the daemon:

1. Snapshots `target_duration_ms` from `wall_started` → stop instant.
2. Stops mic and system recorders in parallel (`join_all`).
3. Collects per-track `track_late_by_ms` and `first_non_silent_at` (wall-clock).
4. Calls `align_meeting_tracks()` → writes timeline-aligned WAVs.

**Mic (dense):** buffer spans the capture window → placed at `track_late_by_ms`.

**System (sparse loopback):** WASAPI often returns only the audible segment. When the buffer is much shorter than expected *and* first content arrived late on the wall clock, samples are copied to `first_content_from_wall_ms` — not t=0. Sub-threshold noise at the buffer head must not disable sparse detection.

See `crates/phoneme-audio/src/meeting_align.rs` and [Meeting Mode](../user-guide/meeting_mode.md).

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
