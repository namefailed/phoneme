# Architecture — how Phoneme works, end to end

This is the day-one read. It follows a recording from the moment you press a
hotkey to the moment it is searchable in your archive, naming every component
along the way and linking into the code that owns it. Where a subsystem
deserves a deeper treatment, this page points at it rather than repeating it:

- [internals.md](internals.md) — subsystem notes (async topology, audio path,
  catalog/search internals, meeting alignment math).
- [backend_guide.md](backend_guide.md) — the Rust workspace map (which crate
  owns what) and the actor/supervision patterns.
- [frontend_guide.md](frontend_guide.md) — the Lit/Store frontend deep dive.
- [technical_challenges.md](technical_challenges.md) — the hard problems behind
  the non-obvious code (the bug, race, or constraint each design decision answers).

Every Rust module also carries its own `//!` doc comment that explains its role
and invariants; the file links below land on those. The crate-level system maps
are the best entry points: [`phoneme-core/src/lib.rs`](../../crates/phoneme-core/src/lib.rs),
[`phoneme-ipc/src/lib.rs`](../../crates/phoneme-ipc/src/lib.rs),
[`phoneme-audio/src/lib.rs`](../../crates/phoneme-audio/src/lib.rs).

---

## 1. The three-process model

Phoneme is not one program. It is a headless **daemon** that owns all the
real work, fronted by two thin clients — a **tray** GUI and a **CLI** — that
talk to it over a local named pipe.

```text
        ┌─────────────────────────┐        ┌─────────────────────────┐
        │  phoneme-tray (Tauri 2) │        │   phoneme (CLI)         │
        │  src-tauri + frontend/  │        │   bin/phoneme           │
        │  tray icon, hotkeys,    │        │   record/list/search/…  │
        │  overlay, settings UI   │        │   scriptable peer       │
        └────────────┬────────────┘        └────────────┬────────────┘
                     │                                  │
                     │  NDJSON requests / responses     │
                     │  + a one-way event subscription  │
                     ▼                                  ▼
        ┌───────────────────────────────────────────────────────────┐
        │  named pipe  \\.\pipe\phoneme-daemon  (owner-only ACL)      │
        └───────────────────────────────┬───────────────────────────┘
                                         │
                                         ▼
        ┌───────────────────────────────────────────────────────────┐
        │  phoneme-daemon  (bin/phoneme-daemon) — the brain          │
        │  microphone · inbox queue · pipeline · SQLite catalog ·    │
        │  event bus · child-process supervision                     │
        └───────┬───────────────────────────────────┬───────────────┘
                │ spawns + supervises               │ spawns on demand
                ▼                                    ▼
     ┌──────────────────────┐            ┌──────────────────────┐
     │  whisper-server.exe  │            │  ollama serve        │
     │  (bundled whisper.cpp│            │  (only if WE start it,│
     │   STT over HTTP)     │            │   ownership ledger)   │
     └──────────────────────┘            └──────────────────────┘
```

**Why split it this way.** The daemon outlives any window. You can quit the
tray, drive everything from the CLI, or run headless on a server — the
recording, transcription, and archive keep working because they live in one
process that owns the hardware and the database. The tray and CLI are
interchangeable front ends:

| Process | Crate / dir | Owns | Doc |
| :--- | :--- | :--- | :--- |
| **Daemon** | [`bin/phoneme-daemon`](../../bin/phoneme-daemon/src/main.rs) | Mic, inbox queue, pipeline, catalog, event bus, child processes | [`main.rs`](../../bin/phoneme-daemon/src/main.rs) |
| **Tray** | [`src-tauri`](../../src-tauri/src/lib.rs) + [`frontend`](../../frontend/src/App.ts) | Window, tray icon, global hotkeys, preview overlay, settings UI (~14 tabs) | [`src-tauri/src/lib.rs`](../../src-tauri/src/lib.rs) |
| **CLI** | [`bin/phoneme`](../../bin/phoneme/src/main.rs) | A scriptable peer for every GUI action | [`bin/phoneme/src/main.rs`](../../bin/phoneme/src/main.rs) |
| **whisper-server** | bundled `whisper.cpp` | Local speech-to-text over HTTP | (external binary) |

Two more clients are **optional** and **off by default**: a loopback-only HTTP+SSE
bridge ([`bin/phoneme-rest`](../../bin/phoneme-rest/src/main.rs)) and a Model
Context Protocol bridge ([`bin/phoneme-mcp`](../../bin/phoneme-mcp/src/main.rs)).
Both front the *same* pipe — one external call maps to one IPC `Request`, returned
verbatim — and neither carries business logic. What each surface exposes (and the
asymmetries between them) is mapped in [feature_parity.md](feature_parity.md).

### The pipe protocol in one paragraph

Clients speak **newline-delimited JSON** ([`phoneme-ipc`](../../crates/phoneme-ipc/src/lib.rs)):
one [`Request`](../../crates/phoneme-ipc/src/schema.rs) object per line, one
[`Response`](../../crates/phoneme-ipc/src/schema.rs) object back, in order, until
the connection closes. The schema doc comments *are* the protocol reference —
every request documents its payload, the daemon's behavior, the exact response
JSON, and the events it triggers. The framing lives in
[`codec.rs`](../../crates/phoneme-ipc/src/codec.rs); the named-pipe transport
(owner-only ACL, first creator wins so a second daemon fails fast) in
[`named_pipe.rs`](../../crates/phoneme-ipc/src/named_pipe.rs). A connection that
sends [`Request::SubscribeEvents`](../../crates/phoneme-ipc/src/schema.rs)
flips into a one-way stream of [`DaemonEvent`](../../crates/phoneme-ipc/src/schema.rs)
lines instead — which is how UIs stay live (see §8). See
[ipc_integration.md](ipc_integration.md) for the wire walkthrough.

---

## 2. Process lifecycle & ownership

A surprising amount of the design is about *who starts what, and who kills it
when things end*. This is the part that keeps a quit from leaving a zombie
whisper-server squatting a port.

### Boot

- **Tray boot** ([`src-tauri/src/lib.rs`](../../src-tauri/src/lib.rs) `run`):
  build the tokio runtime → read config → [`auto_spawn`](../../src-tauri/src/auto_spawn.rs)
  the daemon → try one [`bridge`](../../src-tauri/src/bridge.rs) connect (failure
  is fine; the `BridgeSlot` lazily reconnects on the first action) → register
  global hotkeys → pre-create the hidden overlay → attach the daemon event
  stream → hand the WebView its command surface.
- **CLI boot**: work-creating commands (`record`, `import`, `retranscribe`) use
  the *spawning* path ([`Client::connect`](../../bin/phoneme/src/client.rs) →
  [`auto_spawn`](../../bin/phoneme/src/auto_spawn.rs)) and start a missing
  daemon; read-only commands (`list`, `show`, `search`, `queue`, `watch`,
  `doctor`) use the *observe-only* path and instead report "daemon not running".
- **Version handshake**: a reachable daemon is reused only when its
  `DaemonStatus.version` matches the client build; a stale daemon is restarted —
  except when it reports being mid-recording/mid-transcription, in which case it
  is left to finish so a rolling rebuild can't kill a live capture.

### Kill-on-close job objects (Windows)

Every child the daemon spawns (whisper-server, a Phoneme-launched Ollama) is
assigned to a **kill-on-close Job Object**
([`phoneme-core/src/job.rs`](../../crates/phoneme-core/src/job.rs)). The kernel
reaps the whole tree if the owner dies uncleanly — no orphaned servers. The tray
applies the same trick to the daemon itself: with `interface.quit_stops_daemon`
on (default), the freshly-spawned daemon joins the *tray's* job, so even an
"End task" on the tray takes the daemon (and transitively its children) down.
Windows can't remove a process from a kill-on-close job, so membership is
decided at spawn time — flipping the knob applies to the next spawn.

### The quit shutdown chain

```text
Tray "Quit"  ──(quit_stops_daemon on)──►  send Shutdown ──► poll until pipe gone
   │                                            │
   │                                            ▼
   │                            daemon: ShutdownCoordinator flips one watch flag
   │                            → finalize the in-flight recording via the normal
   │                              stop path (no corrupt WAV) → kill whisper-server(s)
   │                              → stop a Phoneme-launched Ollama → exit
   ▼
tray exits (DAEMON_STOP_DONE flag stops the exit hook re-sending Shutdown)
```

The single [`ShutdownCoordinator`](../../bin/phoneme-daemon/src/shutdown.rs) is a
`watch<bool>` every long-lived task observes; the IPC `Shutdown` handler, the
Ctrl+C listener, and `main`'s failure paths all flip the same flag. The tray
side is in [`tray.rs`](../../src-tauri/src/tray.rs) (`should_stop_daemon_on_exit`
encodes the decision table). With the knob **off**, Quit exits immediately and
the daemon deliberately outlives the tray (the headless contract).

### Ollama ownership ledger

[`ollama_launcher.rs`](../../bin/phoneme-daemon/src/ollama_launcher.rs) starts
`ollama serve` on demand when an LLM step needs it, under one hard rule: **an
Ollama already running when the daemon first probed it is never ours** — never
killed, never restarted, never job-assigned, for the daemon's whole lifetime.
Only a process this module spawned is *Owned*, joined to the kill-on-close job,
and stopped at shutdown. A `next_action` state machine makes the rule
unit-testable; one mutex held across probe→spawn single-flights concurrent LLM
steps so two cleanups can't double-spawn.

### Effective-port fallback

The configured whisper port is a *preference*, not a guarantee. The
[`whisper_supervisor`](../../bin/phoneme-daemon/src/whisper_supervisor.rs)
pre-flight-probes the port; if a foreign process squats it, the supervisor
routes around to a free OS-assigned port (excluding the sibling preview server's
ports so the two never collide), **publishes the chosen port to
[`AppState::whisper_ports`](../../bin/phoneme-daemon/src/app_state.rs) before the
spawn**, and clears it when the server is down. Every consumer resolves
effective-or-configured right where it builds a transcription provider — so the
request always dials the port the server actually bound.

---

## 3. A recording's life

This is the spine of the whole system. Follow one recording from hotkey to
archive.

```text
 hotkey / `phoneme record` / GUI button
        │  (RecordStart | StartMeeting | RecordToggle over the pipe)
        ▼
 ┌─────────────┐   audio @ 16 kHz mono i16, optional pre-roll + live preview
 │  recorder   │   inserts catalog row (status=recording), emits RecordingStarted
 └──────┬──────┘
        │ stop: finalize WAV, flip row → transcribing
        │
        ├──────────────► in-place dictation?  yes → FAST LANE (§5), skip the queue
        │
        ▼ no
 ┌─────────────┐   one JSON file in pending/  (durable, survives a crash)
 │ inbox queue │
 └──────┬──────┘
        ▼ claim one at a time
 ┌─────────────┐
 │ queue_worker│   publishes the in-flight id + cancel token, then:
 └──────┬──────┘
        ▼
 ┌───────────────────────────────────────────────────────────────────┐
 │ pipeline::run  — each optional stage gated by config, non-fatal     │
 │                                                                     │
 │  transcribe ─► clean (LLM) ─► auto-title ─► [type, in-place only] ─► │
 │  embed ─► hooks + keyword hooks ─► summary (LLM) ─► tags (LLM) ─►    │
 │  done + webhook                                                     │
 └──────┬──────────────────────────────────────────────┬──────────────┘
        │ writes results as they settle                 │ broadcasts progress
        ▼                                               ▼
   SQLite catalog                                   event bus  ──► UIs refresh
```

### Trigger → capture

A `RecordStart` / `StartMeeting` / `RecordToggle` request arrives over the pipe
and reaches the daemon [`recorder`](../../bin/phoneme-daemon/src/recorder/mod.rs).
`start` inserts the catalog row (status `recording`), opens the audio source,
and emits `RecordingStarted`. The capture itself is the
[`phoneme-audio`](../../crates/phoneme-audio/src/lib.rs) `Recorder` actor — a
state machine driven over a command channel, resampling everything to the
canonical **16 kHz mono `i16`** ([`recorder.rs`](../../crates/phoneme-audio/src/recorder.rs)).
If pre-roll is enabled, an idle background ring buffer
([`preroll.rs`](../../crates/phoneme-audio/src/preroll.rs)) is snapshotted and
prepended so the first word is never clipped. In `oneshot` mode a silence
detector ([`silence.rs`](../../crates/phoneme-audio/src/silence.rs)) auto-stops.

The recorder owns hard invariants: **at most one capture** (a single recording
OR a two-track meeting, never both), **toggle atomicity** (a double-tapped
hotkey can't race two starts/stops), and **no slow await under a state lock**
(so control IPC stays responsive mid-stop).

**Per-keybind capture source.** `RecordStart` / `RecordToggle` carry an optional
`source_override` (`HotkeyBinding.source`), threaded into `recorder.start(…,
source_override)`. The effective kind is `source_override.unwrap_or(
recording.source)` — a binding can record the mic while another records WASAPI
system loopback, each with its own recipe and model. The recorder writes the
*real* source onto the row's `track` (`"mic"` / `"system"`), which is what the
list's **Source** column and its 🎤/🔊 hover icon read — single recordings no
longer assume "microphone" (older rows with no `track` default to mic). The
override is ignored for meetings (a meeting always records both tracks). One edge
case the code handles: **pre-roll is dropped when the override differs from the
global source** — the idle ring buffer is mic-only audio captured under the
global source, so on a mismatch the buffered samples are discarded and a fresh
stream is opened for the override source ([`recorder.rs`](../../bin/phoneme-daemon/src/recorder/mod.rs) `start`).

### The fast-lane-vs-queue decision

On `stop`, the recorder finalizes the WAV and makes the one branching decision
that defines the rest of the recording's life
([`recorder.rs`](../../bin/phoneme-daemon/src/recorder/mod.rs), `stop`):

```text
fast_lane = active.in_place && !in_place_cfg.full_pipeline
```

- **Fast lane** (an in-place dictation with the default config) → hand off to
  [`in_place::spawn_fast_lane`](../../bin/phoneme-daemon/src/in_place.rs),
  skipping the queue and full pipeline entirely (see §5).
- **Otherwise** → enqueue a JSON payload into the durable inbox. (With
  `full_pipeline` + `type_first`, a type-only pass *also* runs immediately for
  instant typing, while the recording rides the normal queue.)

### Inbox queue → worker

The [`InboxQueue`](../../crates/phoneme-core/src/queue.rs) is filesystem-backed
on purpose: state must survive a crash. Each item is a JSON file, and every
state change is an atomic rename between `pending/` → `processing/` →
`done/`/`failed/`. That gives crash recovery for free — on startup,
[`reconcile`](../../bin/phoneme-daemon/src/reconcile.rs) re-queues anything
stranded in `processing/` and marks orphaned in-progress catalog rows as failed,
so nothing spins forever.

The [`queue_worker`](../../bin/phoneme-daemon/src/queue_worker.rs) is a single
task claiming **strictly one item at a time** — transcription is serial by
construction (the bundled whisper-server handles one request well, many poorly).
It publishes the in-flight id and a fresh cancellation token (what
`CancelProcessing` cancels), runs `pipeline::run`, then emits
`QueueDepthChanged`. Its failure policy: transient STT failures (unreachable /
timeout) requeue the same item with exponential backoff (30 s → 5 min); after a
cap of consecutive misses the item is declared failed so a dead server can't
loop one recording forever. Permanent pipeline errors are already quarantined by
the pipeline.

### Pipeline stages

[`pipeline::run`](../../bin/phoneme-daemon/src/pipeline.rs) is the stage after
the queue. Stage order — each optional stage gated by config and **non-fatal**:

1. **transcribe** (with segments + diarization) — via the configured
   [`TranscriptionProvider`](../../crates/phoneme-core/src/transcription.rs)
   (local whisper.cpp, OpenAI/Groq, Deepgram, AssemblyAI, ElevenLabs, or any
   OpenAI-compatible URL). Speaker labels come from
   [`diarization`](../../crates/phoneme-core/src/diarization.rs).
2. **clean** — optional [`LLM`](../../crates/phoneme-core/src/llm.rs) pass
   (Ollama / OpenAI-compatible / Anthropic) to fix stutters, reformat, translate.
3. **auto-title** — a pure [heuristic](../../crates/phoneme-core/src/title.rs)
   (first meaningful clause), with an optional LLM title on top; an auto title
   never overwrites a user-set one.
4. **type** — *in-place full-pipeline dictations only* (see §5).
5. **embed** — [chunk](../../crates/phoneme-core/src/chunk.rs) the transcript and
   [embed](../../crates/phoneme-core/src/embed.rs) each window for semantic
   search (§6).
6. **hooks + keyword hooks** — run the user's [hook](../../crates/phoneme-core/src/hook.rs)
   subprocess with the transcript on stdin.
7. **summary** — optional LLM summary.
8. **tags** — optional LLM auto-tag *suggestions* (you approve them, §7).
9. **enrichment** — optional LLM passes that extract structured metadata into their
   own catalog tables, each gated by recipe membership:
   - **entities** — typed entities (person / org / topic / term),
     [`extract_entities`](../../bin/phoneme-daemon/src/pipeline.rs) → the `entities`
     table.
   - **chapters** — time-ranged topic chapters snapped to real segment timing,
     [`extract_chapters`](../../bin/phoneme-daemon/src/pipeline.rs) /
     [`parse_chapters`](../../bin/phoneme-daemon/src/pipeline.rs) → the `chapters`
     table.
   - **tasks** — action items (each with an optional free-text `due_hint` and a
     user-owned `done` flag), [`extract_tasks`](../../bin/phoneme-daemon/src/pipeline.rs)
     → the `tasks` table.

   Each writes the model that produced it (`entities_model` / `chapters_model` /
   `tasks_model`), replaces any prior set wholesale, and is non-fatal. The **same**
   extractors back on-demand IPC handlers — `SuggestEntities` / `SuggestChapters` /
   `SuggestTasks` (the detail pane's **Extract** buttons and the
   `phoneme suggest-entities` / `suggest-tasks` / `chapters` CLI verbs) — so a manual
   run behaves identically to the pipeline.
10. **done + webhook** — flip the row to `done` and POST the
    [payload](../../crates/phoneme-core/src/types.rs) to a configured
    [webhook](../../crates/phoneme-core/src/webhook.rs).

Results land in the catalog as they settle; progress is broadcast as
`PipelineStageChanged` / `LlmActivity` events; the catalog status column tracks
the stages step for step. Key invariants the pipeline owns: **whisper-server
serialization** (the final STT holds the `whisper_sem` permit for its whole
call, so the live preview can never starve it), **one-job model overrides** (a
re-transcribe with a different model swaps it under the permit and a drop guard
restores it on every exit path, never mutating global config), **effective
ports** (§2), **cancellation** (checkpoints between stages finalize a canceled
item), and **transient-vs-permanent failure** handling. The same helpers
(`run_llm_stage`, `generate_summary`, `suggest_tags`, `embed_and_store`) back the
on-demand re-run IPC handlers, so a manual re-run behaves byte-for-byte like the
pipeline.

**Recipe selection (which chain runs).** The stage list isn't hardcoded — it's
the result of `resolve_recipe(cfg, recipe_id)`, which expands a named
[Playbook](../../crates/phoneme-core/src/config.rs) recipe into ordered steps
(falling back to the `default` recipe for an empty or unknown id, never a panic).
A custom hotkey's recipe, the Re-run modal's **Run through** pick (its "Just this
run" scope), and **per-app tone** all reach the pipeline by the *same* path: the
id is stashed per-job in the
`pending_recipe` ledger ([`app_state.rs`](../../bin/phoneme-daemon/src/app_state.rs))
when the job is created, claimed by `pipeline::run` *before* transcription (so a
transcribe failure can't strand a stale entry), and never written to global
config. **Per-app tone** (`[in_place].app_recipes`) is resolved entirely
daemon-side at record start: `DaemonRecorder::start` runs
`InPlaceConfig::resolve_app_recipe` against the foreground app it already snapshots
and seeds the result into that same `pending_recipe` ledger — so a matched app's
dictation routes off the fast lane (via the existing `has_recipe`/`wants_fast_lane`
check) and runs its recipe with **no new IPC, request, or event**. A custom
hotkey's own recipe wins: it is stashed *after* `start()` returns and overwrites
the per-app seed when non-empty. The Re-run modal's **Advanced** disclosure (under
"Just this run") layers one-time per-step model overrides *on top of* whichever
recipe you pick — only the cleanup/title/summary steps that recipe actually runs —
and `apply_rerun_overrides` mutates the matching Playbook entries on a per-job
config **clone** (the executor reads each step's model/prompt from its entry),
then the clone is discarded.

### Catalog & UI refresh

Every result is written to the SQLite [`Catalog`](../../crates/phoneme-core/src/catalog/mod.rs)
(WAL mode, FTS5 full-text index, per-chunk embedding vectors). As stages
complete, the [`event_bus`](../../bin/phoneme-daemon/src/event_bus.rs) broadcasts
`DaemonEvent`s; every subscribed client follows along (§8). For schema details,
see [internals.md](internals.md#sqlite-catalog--search-internals).

Each stage also persists the provider+model it actually ran with, so the detail
pane's footer **🪈 Pipeline** button opens a provenance popover listing every
stage and the model that produced it — exactly what transcribed, cleaned,
titled, summarized, and tagged this recording (including a re-run's one-off
overrides). Those per-step model names are indexed too, so a library search
matches on them: searching a model name surfaces every recording that ran
through it.

---

## 4. The dictation / in-place path (fast lane)

Pressing the in-place hotkey (default `Ctrl+Alt+I`) dictates straight into the
focused application. The whole point is **latency** — text should appear the
moment you stop talking — so this path deliberately skips the queue and the full
pipeline.

[`in_place.rs`](../../bin/phoneme-daemon/src/in_place.rs) flow:

```text
transcribe (dictation provider) ─► polish ─► type/paste at cursor ─► persist (background)
```

- **Polish** is rule-based by default ([`dictation.rs`](../../crates/phoneme-core/src/dictation.rs)):
  filler stripping, stutter collapse, capitalization — *zero* latency, no LLM
  round-trip unless `cleanup = "llm"`.
- **Type vs paste** (`type_at_cursor`): in `"paste"` mode it goes via the
  clipboard (set → Ctrl+V → restore the previous clipboard) — near-instant for
  long text; otherwise it sends simulated keystrokes via `enigo`, which works in
  apps that block paste. Input APIs run on a blocking thread.
- **Persist last**: the recording is written to the library *after* the text has
  already landed, off the latency path.

A dictation never waits behind a meeting that's mid-transcription and never runs
diarization. Stage events still fire (Transcribing → Done/Failed), so the queue
panel, status column, and step notifications track a dictation exactly like a
queued recording. With `full_pipeline` + `type_first`, a second type-only pass
([`spawn_type_first`](../../bin/phoneme-daemon/src/in_place.rs)) handles the
instant typing while the recording itself rides the normal queue — the pipeline
owns every catalog write and skips its own typing so the text never lands twice.
User-facing details: [transcribe_in_place.md](../user-guide/transcribe_in_place.md).

---

## 5. Meeting mode — dual-track capture

A meeting captures **two** tracks that share one wall-clock timeline: a dense
**microphone** track (you) and a sparse **WASAPI system loopback** track
(everyone else, straight off the speakers). They share a `meeting_id`.

```text
   microphone ──► track A (dense)   ┐
                                    ├─ recorded concurrently, one meeting_id
   system loopback (WASAPI) ──► B   ┘            │
                                                 ▼ on stop: wall-clock align
                                    meeting_align: pad sparse loopback with
                                    leading silence to its true start time
                                                 │
                                                 ▼ each track transcribed separately
                                    per-track segments ──► merged chronological view
```

### Why alignment is non-trivial

Windows only delivers loopback packets *while sound is actually playing*. When
the call is quiet, the loopback device sends no frames, so its buffer ends up
shorter than the mic's. [`meeting_align.rs`](../../crates/phoneme-audio/src/meeting_align.rs)
detects this *sparse* state (significant duration deficit + first loud block
after the meeting started) and pads the loopback's start with silence matching
its wall-clock offset, so both tracks line up on a single timeline. The math is
in [internals.md](internals.md#dual-track-alignment-math).

Each track is transcribed independently, and the detail view merges both tracks'
segments into one chronological transcript (the merged view maps `[Speaker N]`
to per-track custom names). Optional speaker diarization (offline ONNX via
`speakrs`, or a cloud provider) labels who spoke. User-facing details:
[meeting_mode.md](../user-guide/meeting_mode.md).

### Live preview overlay

While recording, an optional always-on-top [`overlay`](../../src-tauri/src/overlay.rs)
window floats partial transcripts over the whole desktop. The recorder runs a
preview loop that transcribes a rolling tail window and emits
`TranscriptionPartial`; it only ticks when the shared `whisper_sem` permit is
free, so it can never starve a final transcription. The overlay needs nothing
special to receive this — [`events`](../../src-tauri/src/events.rs) re-emits every
daemon event to all webviews, and `overlay.ts` drives show/hide from the
recording events. See [streaming_preview_and_preroll.md](../user-guide/streaming_preview_and_preroll.md).

---

## 6. The recall path — semantic + keyword search

Phoneme finds a recording whether you remember its *gist* or its one distinctive
*word*. It fuses two retrievers.

```text
                    new transcript                    user query
                          │                                │
                          ▼ chunk (sentence-aware,         ▼ embed query
                          │  ~few-sentence overlapping     │  (same model)
                          │  windows)                      │
                          ▼ embed each window              │
                    embedding_chunks (f32 BLOBs in SQLite) │
                          │                                │
       ┌──────────────────┴────────────┐                  │
       ▼                                ▼                  ▼
  Lexical (FTS5)                  Vector (per-chunk cosine, max-sim)
  exact terms, prefix match       brute-force scan over the corpus
       │                                │      (in-memory embedding cache)
       └────────────────┬───────────────┘
                        ▼
            Reciprocal Rank Fusion (RRF)
            score += w / (k + rank), k=60
            vector weight 1.0, lexical 0.85
                        │
                        ▼
            calibrate cosine → relevance %
            (≤0.15 → 0%, ≥0.70 → 100%, linear between)
                        │
                        ▼
            de-dup on a meeting-stable key (a meeting's
            two tracks collapse to one result)
```

- **Chunking** ([`chunk.rs`](../../crates/phoneme-core/src/chunk.rs)): the
  embedding model truncates at ~256 tokens and mean-pools to one vector, so
  embedding a whole transcript drops the back half and smears distinct ideas
  together. Splitting into overlapping sentence-aware windows lets one spoken
  idea compete on its own tight vector.
- **Embedding** ([`embed.rs`](../../crates/phoneme-core/src/embed.rs)): an ONNX
  sentence-transformer (bundled `all-MiniLM-L6-v2`, 384-dim, L2-normalized so
  cosine is a dot product). The knobs (pooling, max length, prefixes) are
  config-driven so you can swap in E5/BGE/GTE/MPNet.
- **Embedding cache** ([`catalog/embeddings.rs`](../../crates/phoneme-core/src/catalog/embeddings.rs)):
  the cosine scan is brute-force over every stored vector, so the catalog holds
  the decoded corpus in memory (`Arc<RwLock<…>>`, shared across clones) instead
  of re-reading and re-decoding f32 BLOBs from disk every query. Invalidation is
  coarse and pessimistic — any embedding write or a delete drops the snapshot and
  the next query rebuilds it. Above a 200k-vector cap it stays uncached and reads
  from SQLite, keeping memory bounded.
- **Fusion** ([`fusion.rs`](../../crates/phoneme-core/src/fusion.rs)): RRF
  combines the two ranked lists without needing their score scales to be
  comparable; an item ranked well by *either* retriever floats up, by *both*
  floats highest. Then cosine is calibrated into the relevance percentage the UI
  shows.

### More-like-this

Given a stored recording, "more like this" asks the daemon for its semantic
neighbours — `Request::MoreLikeThis`, scored against the recording's
**already-stored** vectors, so **no query embedding happens at all**. It works
even when the embedding model isn't loaded, and is essentially free. The tray
command is [`similar.rs`](../../src-tauri/src/similar.rs); the CLI is
`phoneme search --like <ID>`. Both return the same `[{recording, score}]` shape
as a text search. Internals: [internals.md](internals.md#semantic-hybrid-search--fusion);
user-facing: [semantic_search.md](../user-guide/semantic_search.md).

---

## 7. Auto-tagging & approval

The auto-tag stage suggests metadata tags from transcript content **without
auto-applying them** (preventing tag clutter). The daemon reads existing catalog
tags ([`tags.rs`](../../crates/phoneme-core/src/tags.rs)) and prompts the LLM to
prefer existing tags before inventing new ones. Suggestions are stored as a JSON
array (`tag_suggestions`) and surface as dashed tag chips. Approving promotes a
suggestion to a permanent `recording_tags` relationship (creating the tag entity
if needed); dismissing clears them. If `auto_apply` is enabled, suggestions that
match existing tag labels attach immediately. CLI parity: `phoneme suggest-tags`,
`phoneme tag`. See [auto_tagging.md](../user-guide/auto_tagging.md).

---

## 8. Communication — the event bus & live UI

Two halves to the protocol (both over the same NDJSON named pipe):

- **Request/response** — every command. The tray funnels all WebView `invoke`
  calls through one [`Bridge`](../../src-tauri/src/bridge.rs) connection (mutex
  serialized, transparent reconnect-and-retry); the CLI dials a fresh connection
  per invocation.
- **Event stream** — a `SubscribeEvents` connection becomes a one-way stream of
  [`DaemonEvent`](../../crates/phoneme-ipc/src/schema.rs)s from the
  [`event_bus`](../../bin/phoneme-daemon/src/event_bus.rs) (a 64-slot tokio
  broadcast channel). Delivery is fire-and-forget: a subscriber that lags more
  than the buffer is disconnected and expected to reconnect and re-fetch state,
  which is why no daemon code path ever *depends* on an event being delivered.

The tray's [`events`](../../src-tauri/src/events.rs) opens a **dedicated**
subscription connection (separate from the request bridge), and for every event
(1) derives a fresh [`TrayState`](../../src-tauri/src/tray.rs) for the tray icon
and (2) re-emits the event verbatim as the Tauri `daemon-event`, which broadcasts
to all webviews. The frontend stores refresh from it and the overlay drives its
show/hide. When the stream ends (daemon restart, lag), it reconnects on a 2 s
loop — re-subscribing also satisfies the "re-fetch after lag" contract. The
frontend side of this flow is in [frontend_guide.md](frontend_guide.md).

The CLI's `phoneme watch` is the scripting counterpart: it prints every
`DaemonEvent` as raw JSON lines (pipe through `jq`); blocking `phoneme record`
subscribes first, then waits for its recording's `TranscriptionDone`.

---

## 9. Doctor & self-healing

[`doctor.rs`](../../crates/phoneme-core/src/doctor.rs) is the shared health-check
implementation; the GUI dashboard and `phoneme doctor` run the **same** probes
([`src-tauri/src/doctor.rs`](../../src-tauri/src/doctor.rs) just re-exports it).
`run_local_checks` is synchronous (config presence, audio-dir writability, disk
space, hook resolvability, model integrity); `run_backend_checks` is async and
probes remote endpoints with short timeouts. Both are **provider-aware** — every
check follows the *effective* connection a feature will actually use (main STT,
live preview, dictation override, each enabled LLM step). Local providers keep
model-file and supervised-server checks; cloud providers swap them for "the key
is set and the endpoint answers" without sending a billable request. Each result
carries a `CheckCategory` (severity) and, for the GUI, a `fix_action` so a click
(or `phoneme doctor --fix`) sweeps a hung/orphaned whisper-server and respawns it
from config. The supervisor's explicit `whisper_restart` notify is the only path
that heals a *hung* (not just dead) server. See [troubleshooting.md](../user-guide/troubleshooting.md).

### Diagnostics bundle

Doctor's **Export diagnostics** button (the `ExportDiagnostics` IPC request) writes
an opt-in, **local-only** sanitized snapshot for a bug report
([`diagnostics.rs`](../../crates/phoneme-core/src/diagnostics.rs)): app/version/OS
info, the **masked** config (every secret redacted through the shared
`phoneme_core::secrets` layer — never a plaintext key), and a tail of the daemon
log. It deliberately includes **no audio, no transcripts, no catalog contents, and
makes no network call** — the daemon assembles it from disk plus in-memory config and
writes it to `<data_dir>/diagnostics/phoneme-diagnostics-<timestamp>.json`, returning
the path for the UI to reveal. The user chooses whether to share the file.

---

## 10. Workspace at a glance

| Crate / dir | Owns | Entry point |
| :--- | :--- | :--- |
| [`phoneme-core`](../../crates/phoneme-core/src/lib.rs) | Domain logic & data: config, transcription/LLM/diarization providers, catalog (SQLite + FTS5 + embeddings), chunk/embed/fusion, hook/webhook, doctor, queue, types | [`lib.rs`](../../crates/phoneme-core/src/lib.rs) |
| [`phoneme-audio`](../../crates/phoneme-audio/src/lib.rs) | Capture & encoding: device enum, recorder state machine, resample, silence/pre-roll, WAV/decode, meeting alignment | [`lib.rs`](../../crates/phoneme-audio/src/lib.rs) |
| [`phoneme-ipc`](../../crates/phoneme-ipc/src/lib.rs) | The wire contract: schema, NDJSON codec, named-pipe transport | [`schema.rs`](../../crates/phoneme-ipc/src/schema.rs) |
| [`phoneme-agent-core`](../../crates/phoneme-agent-core/src/lib.rs) | The agent tool seam: the single tool catalog mapping each tool + schema to one IPC `Request` (drives `phoneme-mcp`) | [`lib.rs`](../../crates/phoneme-agent-core/src/lib.rs) |
| [`phoneme-daemon`](../../bin/phoneme-daemon/src/main.rs) | The brain: IPC server/handler, recorder, inbox worker, pipeline, supervisors, event bus | [`main.rs`](../../bin/phoneme-daemon/src/main.rs) |
| [`phoneme` (CLI)](../../bin/phoneme/src/main.rs) | Scriptable peer: one module per subcommand | [`main.rs`](../../bin/phoneme/src/main.rs) |
| [`phoneme-rest`](../../bin/phoneme-rest/src/main.rs) | Optional loopback HTTP+SSE bridge (off by default): one HTTP call → one IPC `Request` | [`main.rs`](../../bin/phoneme-rest/src/main.rs) |
| [`phoneme-mcp`](../../bin/phoneme-mcp/src/main.rs) | Optional MCP bridge (off by default): JSON-RPC over stdio, tools from `phoneme-agent-core` | [`main.rs`](../../bin/phoneme-mcp/src/main.rs) |
| [`src-tauri`](../../src-tauri/src/lib.rs) | Tray shell: spawn/bridge the daemon, command forwards, event re-emit, tray/overlay/wizard | [`lib.rs`](../../src-tauri/src/lib.rs) |
| [`frontend`](../../frontend/src/App.ts) | Lit SPA: views, stores, services, keyboard system | [`App.ts`](../../frontend/src/App.ts) |

For the deeper Rust map (actor pattern, semaphores, supervision, SQLx/WAL),
read [backend_guide.md](backend_guide.md). For subsystem internals (async task
topology, audio details, catalog/search internals, alignment math), read
[internals.md](internals.md). For the frontend, read [frontend_guide.md](frontend_guide.md).
