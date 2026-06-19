# Internals — subsystem deep dives

This page is the companion to [architecture.md](architecture.md). The
architecture page is the canonical end-to-end journey (hotkey → typed text,
recording → searchable archive); this page goes one level deeper on the
subsystems that journey touches. If you want the *story*, start there; if you
want the *mechanics* of a particular piece, you're in the right place. For the
*hard problems* — the bugs, races, and constraints behind the non-obvious code —
see [Technical Challenges & Engineering Decisions](technical_challenges.md).

Every subsystem here is owned by a Rust module whose `//!` doc comment is the
authoritative description — the links below land on it.

- [Async task topology](#async-task-topology)
- [Audio path](#audio-path)
- [SQLite catalog & search internals](#sqlite-catalog--search-internals)
- [Semantic hybrid search & fusion](#semantic-hybrid-search--fusion)
- [Dual-track alignment math](#dual-track-alignment-math)

---

## Async task topology

The daemon is a **Tokio** runtime coordinating several long-lived tasks. `main`
([`main.rs`](../../bin/phoneme-daemon/src/main.rs)) wires them: load config →
build [`AppState`](../../bin/phoneme-daemon/src/app_state.rs) → recover crash
leftovers ([`reconcile`](../../bin/phoneme-daemon/src/reconcile.rs)) → spawn the
queue worker, both whisper supervisors, the retention loop, and the embedding
backfill → serve IPC until shutdown.

```text
  \\.\pipe\phoneme ──accept──► ipc_server ──spawn per client──► ipc_handler task
                                                                     │
                       request/response                             │ commands
                                                                     ▼
                                                            recorder actor ◄── mpsc
                                                                     │
                                                  stop: enqueue ─────┤
                                                                     ▼
                                            inbox queue (on disk) ──claim──► queue_worker
                                                                              │
                                                                              ▼
                                                                         pipeline::run
                                                                          │        │
                                                          writes ─────────┤        ├──► event_bus (broadcast)
                                                                          ▼                 │
                                                                   SQLite catalog            │ fan-out
                                                                                             ▼
                                                                       ipc_handler ──NDJSON──► subscribed clients
```

**Crash discipline.** The IPC serve loop `select`s on the queue-worker and
main-supervisor join handles, so a crashed critical task takes the whole daemon
down (children die with the kill-on-close job) rather than leaving a zombie that
accepts requests it can never serve. The *preview* supervisor is deliberately
**not** in that select — a preview crash must not kill the daemon — but its
handle is awaited on shutdown so its server never outlives us. See
[`main.rs`](../../bin/phoneme-daemon/src/main.rs).

### Channel types

| Primitive | Where | Purpose |
| :--- | :--- | :--- |
| `mpsc` | [`recorder`](../../crates/phoneme-audio/src/recorder.rs) | Commands to the recorder actor (`Stop`, `Cancel`, `Pause`, `Resume`, `Snapshot`) |
| `oneshot` | recorder actor | Request/response with the actor (grab a `Snapshot`, the final `on_done` signal) |
| `broadcast` | [`event_bus`](../../bin/phoneme-daemon/src/event_bus.rs) | Fan-out `DaemonEvent` to every subscriber (64-slot, fire-and-forget) |
| `watch<bool>` | [`shutdown`](../../bin/phoneme-daemon/src/shutdown.rs) | One process-wide shutdown flag every loop observes |

The **actor pattern** behind the recorder and the **semaphore** discipline
(`whisper_sem` serializing the bundled server between final transcription and
live preview) are covered in [backend_guide.md](backend_guide.md#2-tokio-async-architecture--actors).
The lifecycle/ownership/shutdown story is in
[architecture.md](architecture.md#2-process-lifecycle--ownership).

---

## Audio path

Everything audio converges on the canonical format: **16 kHz, mono, signed
16-bit PCM** (`i16`). Source: [`phoneme-audio`](../../crates/phoneme-audio/src/lib.rs).

- **Sources** ([`source.rs`](../../crates/phoneme-audio/src/source.rs)): the
  `Source` trait abstracts capture hardware. `CpalSource` handles microphone
  input and WASAPI system loopback; `SyntheticSource` feeds predetermined
  waveforms in tests so the pipeline runs on headless CI with no audio hardware.
- **Resampling** ([`convert.rs`](../../crates/phoneme-audio/src/convert.rs)):
  high-quality resampling via `rubato`.
- **Silence detection** ([`silence.rs`](../../crates/phoneme-audio/src/silence.rs)):
  RMS amplitude over a sliding window. In `oneshot` mode, if RMS stays below the
  threshold for the configured duration, the recorder auto-stops.
- **Pre-roll** ([`preroll.rs`](../../crates/phoneme-audio/src/preroll.rs)): a
  ring buffer of the last *N* ms of audio. Idle mic capture runs in the
  background recycling samples; on a record trigger the buffer is prepended so
  the first word is never clipped. Pre-roll is **mic-only and captured under the
  global `[recording].source`**, so when a per-keybind capture-source override
  (below) selects a *different* source, the daemon discards the buffered samples
  and opens a fresh stream for the override source — the recording captures (and is
  labelled as) the source the binding asked for, never a stale device's audio.
- **Per-keybind capture source**: `recorder.start` takes a `source_override`
  (`Option<CaptureSource>`) from the firing `HotkeyBinding.source` and resolves
  the effective kind as `source_override.unwrap_or(recording.source)`. It opens
  the mic (`CpalSource`) or WASAPI loopback accordingly and stores the result on
  the row's `track`, so one hotkey can record the mic and another system audio.
  The override is ignored for meetings (both tracks always recorded).
- **Import decode** ([`decode.rs`](../../crates/phoneme-audio/src/decode.rs)):
  `symphonia` decodes and resamples external files (`.wav`/`.mp3`/`.m4a`/`.flac`),
  behind a size-limit guard against memory exhaustion from a malicious or corrupt
  file.

The capture state machine itself ([`recorder.rs`](../../crates/phoneme-audio/src/recorder.rs))
is the actor described in [backend_guide.md](backend_guide.md#2-tokio-async-architecture--actors).

---

## SQLite catalog & search internals

The catalog lives in `catalog.db` under local app data
([`catalog.rs`](../../crates/phoneme-core/src/catalog.rs)).

### Connection settings

- **WAL mode** (`journal_mode=WAL`): readers query concurrently while a write
  transaction is in progress.
- **Synchronous Normal** (`synchronous=NORMAL`): skips an fsync on every
  transaction while staying crash-safe in WAL mode.
- **Bounded WAL** (`wal_autocheckpoint=1000`, ~4 MB): the daemon also calls
  `Catalog::checkpoint` on idle to reclaim disk, because a long-running read
  connection can otherwise block auto-checkpointing and let the WAL grow without
  bound.

### Tables

- **`recordings`** — the primary row. Notably it keeps **three transcript
  layers** so nothing is lost: `original_transcript` (raw ASR), `clean_transcript`
  (pipeline LLM output), and the live `transcript` (hand-editable). A hand edit is
  reversible because the machine layers are preserved. The `track` column records
  the **actual capture source** — `"mic"` / `"system"` for a single recording (set
  from the effective source at `recorder.start`, including a per-keybind override),
  or the meeting track label; the list's Source column reads it, and a null `track`
  on an older row renders as Microphone.
- **`recordings_fts`** — an FTS5 virtual table indexing transcripts, kept in sync
  by SQLite triggers on insert/update/delete.
- **`tags` & `recording_tags`** — many-to-many categorization.
- **Segments** — stored separately and replaced wholesale on every
  (re)transcribe, so user edits to `transcript` never rewrite the timeline.
- **`embedding_chunks` / `embeddings`** — per-chunk f32 BLOB vectors for semantic
  search (see below).
- **`ai_activity`** — completed streaming LLM sessions (cleanup/summary and their
  re-runs), each row holding the exact prompt and response. The AI-activity log is
  *persisted* here, not transient-event-only, so the 🧠 popout's history survives a
  daemon/app restart; `list_ai_activity` reads it back (newest first), and the
  daemon prunes the table to a bounded recent window.

### Status is a string column

[`RecordingStatus`](../../crates/phoneme-core/src/types.rs) round-trips through
stable lowercase strings (`"transcribing"`, `"hook_failed"`, …). A status the
parser doesn't know errors the whole query, so every variant must have an arm.

### FTS5 query sanitizing

User queries are sanitized in [`catalog.rs`](../../crates/phoneme-core/src/catalog.rs)
so dangling quotes/operators can't crash the SQLite query engine: non-alphanumeric
characters are stripped and terms become prefix matches joined with `AND` (e.g.
`"data migration"` → `data* AND migration*`).

### Embedding cache

The cosine scan is brute-force over every stored vector, so re-`SELECT`ing and
re-decoding the whole corpus from disk per query dominates the cost. The catalog
holds the **decoded corpus in memory** behind `Arc<RwLock<Option<EmbeddingCorpus>>>`,
shared across the clones the daemon hands its workers. Invalidation is coarse and
pessimistic — every embedding write (`upsert_embedding`, `upsert_chunk_embeddings`,
`clear_all_embeddings`) and a recording delete drops the snapshot; the next query
rebuilds it from SQLite. Above a fixed 200k-vector cap the corpus is left uncached
(reads fall back to SQLite per query), keeping memory bounded no matter how large
the archive grows.

---

## Semantic hybrid search & fusion

Phoneme fuses **lexical** (keyword) matching with **semantic** (concept)
matching. The full pipeline diagram and the *why* are in
[architecture.md](architecture.md#6-the-recall-path--semantic--keyword-search);
this section is the math.

### Sentence-aware chunking

Long recordings are split into overlapping sentence-aware windows
([`chunk.rs`](../../crates/phoneme-core/src/chunk.rs)). This avoids two failures:
the embedding model truncates at ~256 tokens (so a long transcript's back half
would be unsearchable), and mean-pooling a long passage smears distinct ideas
into one averaged vector (so a query paraphrasing *one* sentence barely moves the
cosine). At query time a recording is scored by its **best-matching** chunk
(max-sim).

### Reciprocal Rank Fusion (RRF)

RRF ([`fusion.rs`](../../crates/phoneme-core/src/fusion.rs)) merges the two ranked
lists without needing their score scales to be comparable (cosine ∈ ~`[0,1]` vs
unbounded, sign-flipped BM25):

```text
score(d) = Σ over retrievers m of  w_m / (k + rank_m(d))
           k = 60,  w_vector = 1.0,  w_lexical = 0.85
```

An item ranked highly by *either* retriever floats up; ranked well by *both*,
highest. This is more robust than the old single hard cosine floor, which
silently dropped genuine paraphrase hits sitting just under the threshold.

### Relevance calibration

Raw cosine from `all-MiniLM-L6-v2` clusters between ~`0.3` (unrelated) and
~`0.75` (exact). A linear ramp calibrates it into the user-facing percentage:
cosine `≤ 0.15` → **0%**, `≥ 0.70` → **100%**, linear between.

### De-duplication

Results are de-duplicated on a meeting-stable key, so a meeting's two tracks
collapse to one result rather than appearing twice.

---

## Dual-track alignment math

In meeting mode, Phoneme records two WAV files: a **dense** microphone track and
a **sparse** WASAPI system-loopback track. The architectural overview is in
[architecture.md](architecture.md#5-meeting-mode--dual-track-capture); this is the
reconstruction logic in [`meeting_align.rs`](../../crates/phoneme-audio/src/meeting_align.rs).

### Why loopback is sparse

Windows only sends audio packets to the system loopback device when a sound is
*actually playing*. When the call is quiet the loopback delivers no frames, so the
captured loopback buffer ends up shorter than the microphone buffer.

### Reconstruction & padding

`meeting_align` aligns both tracks onto a single wall-clock timeline:

1. The loopback track records the offset of its first audible block.
2. The **sparse** classification fires when the loopback buffer has a significant
   duration deficit *and* the first loud sound occurred after the meeting started.
3. The sparse loopback buffer is padded at the front with silence matching its
   wall-clock start time, lining it up with the microphone track.

After alignment, each track transcribes independently and the detail view merges
their segments chronologically.
