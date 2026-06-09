# 🗺️ Phoneme Roadmap

The single source of truth for **where Phoneme is going**. Shipped history lives in
[`CHANGELOG.md`](CHANGELOG.md); speculative / unvetted ideas live in the
[Idea Parking Lot](docs/IDEAS.md).

---

## Who we build for

Phoneme isn't one app — it's four overlapping workflows. Every roadmap item should
move at least one of these personas closer to "done":

| Persona | Core job | "Done" feels like |
|---|---|---|
| **Dictator** | Hotkey → speak → text lands in Slack/Word/IDE | Zero friction, never think about modes |
| **Meeting archivist** | Capture a call + your reactions → readable timeline → summary | One chronological story, not two blobs |
| **Recall librarian** | Find "that thing I said about Rust errors last week" | Meaning-search + organization that scales past 5k rows |
| **Automator** | Every recording fires Obsidian/Discord/scripts | Reliable, debuggable, configurable without editing TOML |

## Guiding principles

1. **"Would a real user hit this friction?"** — features that duplicate existing
   functionality or serve <~10% of users get cut or parked, not built.
2. **Missing buttons, not missing features.** The backend + CLI are consistently
   *ahead of the GUI*. A large share of high-value work is **wiring + polish**, not
   greenfield engineering (webhook config, semantic scores, failed-queue, delete
   modes, catalog rebuild all exist in the backend with no UI). Close that gap first.
3. **Differentiation lives in Recall and Meetings**, not Dictation. Dictation is
   table-stakes (keep it frictionless); the moat is "search/ask your own voice
   archive" and "dual-track diarized meetings."
4. **Respect dependencies.** Some "separate" features share one substrate — build
   the substrate once (see the ⚠️ callouts below).

---

## 🔧 In flight — v1.8.x (correctness & performance)

Landing now as focused PRs (each tested, `clippy -D warnings` clean):

- [x] **Live preview no longer O(n²)** — the streaming preview re-transcribed the
  *entire* growing buffer every tick; now bounded to a rolling 15 s window.
- [x] **Diarization speaker mapping fixed** — pyannote `"SPEAKER_00"` labels were
  `parse::<u8>()`'d and collapsed everyone to one speaker; now mapped to stable
  indices with gap handling, and run off the async runtime.
- [x] **Semantic search hardened** — dimension check, relevance floor (drop noise),
  and meeting-track dedupe.
- [x] **Embedding input truncated** to the model's 256-token limit (long transcripts
  silently failed to embed → became unsearchable).
- [x] **In-place dictation surfaces errors** instead of panicking / silently no-op'ing.
- [x] **Meeting tracks stay synced across silence** — loopback (system audio) is
  captured continuously by filling real silence for wall-clock gaps, so pausing
  a video mid-meeting no longer collapses the gap and desyncs the tracks; the
  fill is pause-aware (a meeting pause freezes both tracks, no back-filled silence).
- [x] **Transcriptions no longer time out under live preview** — a whisper-server
  semaphore makes the preview yield to the real transcription, fixing the
  `"Whisper timed out after 60s"` failures on long recordings / meetings.

> These are the baseline for v1.9 — several items below build directly on them.

---

## 🔒 Security hardening (from June 2026 audit)

*Posture today: solid app-layer hygiene (parameterized SQL, FTS5 sanitization,
shlex hooks with timeouts, redacted Debug, no `unsafe`), but a weak platform
trust boundary. Fit for a single trusted Windows user; not hardened against
same-user malware or a malicious IPC client. Ordered by priority.*

**Near-term (the trust boundary)**
- [x] **Named-pipe access control** — owner-only SDDL (`D:P(A;;GA;;;OW)(A;;GA;;;SY)`)
  on every pipe instance removes the default cross-session `GENERIC_READ` (which
  exposed the transcript event stream). Same-user isolation (an auth token)
  remains open. *(audit S-C1/S-H8)*
- [ ] **Hook execution allowlist over IPC** — `RefireHook`/`HookTest` accept
  arbitrary commands; restrict to the hooks already in config rather than
  running anything a caller sends. *(S-C2)*
- [x] **IPC frame size cap** (`codec.rs`) — NDJSON frames are bounded at 8 MiB;
  an unterminated over-cap frame errors instead of growing the buffer. *(S-H6)*
- [x] **Path guards** — `reveal_file` restricts the target to `audio_dir`;
  audio deletion rejects `..` and paths outside `audio_dir`. *(S-H3/S-H5)*
- [x] **`escapeHtml` the RecordingDetail error path** (`RecordingDetail.ts:59`). *(S-medium)*

**Secrets & transport**
- [ ] **Stop sending full API keys to the WebView** — `read_config` returns
  plaintext keys; serve a masked DTO and keep secrets daemon-side. *(S-H2)*
- [ ] **Encrypt secrets at rest** (Windows DPAPI) instead of plaintext `config.toml`. *(S-H2)*
- [ ] **Webhook SSRF guard** — HTTPS-only, block private/loopback ranges; HMAC
  signing later. *(S-H1)*
- [ ] **Baseline CSP + narrowed asset/fs scopes** (`tauri.conf.json` is `csp:null`,
  `$HOME/**`). *(S-H4 — also tracked under Long Term → Security)*
- [ ] **Model-download checksums** — pin SHA-256 before extracting the whisper zip. *(S-H7)*

**Hygiene**
- [x] **`cargo audit` + `pnpm audit` in CI** (non-blocking advisory job; gate core crates later). *(also in tech-debt backlog)*
- [ ] Hook `HookTest` stderr may contain secrets — redact before returning.
- [x] A short **threat-model doc** capturing these boundaries. → [docs/developer-guide/threat_model.md](docs/developer-guide/threat_model.md)

---

## 📋 v1.9 — Completeness & Recall

**Theme: close the promise-vs-reality gaps and finish the attic.** Most of this is
wiring features the backend already supports. Meetings-first, because the docs
already advertise a merged timeline we don't ship yet (the biggest trust gap).

### ⚠️ Prerequisites / shared infrastructure (do these first)

- [ ] **Meeting-track alignment correctness** — the merged timeline is *not* pure UI
  wiring; it depends on the two tracks being truly time-aligned. The current
  `meeting_align.rs` heuristic is fragile (it can collapse internal silence when the
  system-audio loopback drops gaps). An interleaved timeline built on mis-aligned
  tracks is *worse* than two stacked panes. **Solidify alignment before the merged
  view.**
- [ ] **Word-level timestamps** — shared substrate for transcript↔waveform sync,
  confidence highlighting, *and* tighter diarization boundaries. Do it once; it
  unlocks three features.

### 🏚️ Finish the attic (backend exists, GUI doesn't)

- [ ] **Webhook URL field in Settings** — pipeline already POSTs `hook.webhook_url`; no UI field exists.
- [ ] **Failed-queue visibility + retry** — inbox has a `failed/` state; the header badge only counts pending+processing. Surface failures with per-file error + one-click retry.
- [ ] **Doctor: rebuild catalog** — `phoneme doctor --rebuild-catalog` is CLI-only.
- [ ] **Delete modes (keep-audio / transcript-only)** — CLI supports it; the GUI always deletes both.
- [ ] **Bulk tag from multi-select** — bulk bar only has re-transcribe / export / delete.
- [ ] **Semantic search settings + re-index** — the wizard sets it up; there's no ongoing management (toggle, model dir, backfill/re-index button).
- [ ] **IPC reconnect after Doctor "Fix"** — today users must close/reopen the window after a daemon restart.
- [ ] **In-app hook log tail** — hook debugging means opening `%LOCALAPPDATA%\phoneme\logs\hook.log` by hand.
- [ ] **Import file picker** — drag-drop works; the picker (`pickAndImportAudio`) needs verifying/wiring. *(Verify current state — may be "moved to Settings," not missing.)*
- [ ] **FLAC import** — docs mention FLAC; the decoder only accepts wav/mp3/m4a.
- [ ] **Recording mode on the main button** — hotkeys support hold/toggle/duration; the header Record button is always one-shot.

### 🎙️ Meetings

- [ ] **Chronological merged timeline** — interleave the two tracks into one "You / Meeting" story. *(Depends on alignment correctness above. Build it in Lit.)*
- [ ] **Diarization quality** *(prereq for named speakers — don't build naming UX on wrong labels)*:
  - [ ] Cache the diarization model in `AppState` (today it reloads the ~500 MB model every recording).
  - [ ] Word-level alignment instead of 1 s segments.
  - [ ] Speaker-count control + better clustering.
- [ ] **Named speakers** — rename "Speaker 1" → "Sarah" once, persisted across exports. *(After diarization quality lands.)*
- [ ] **Meeting capture profiles** — one click "Standup" (tag + summarize preset + Obsidian hook) vs "Interview" (diarize on, different prompt). Config profiles exist; tie them to capture intent.
- [ ] **Post-meeting digest** — meeting ends → optional "Summarize now?" with a one-click LLM preset.

### 🔎 Recall

- [ ] **Show semantic relevance scores in the list** — the IPC already returns `score`; the UI discards it. Now that v1.8 added a relevance floor, showing "87% match" also *explains* why a vague query returns few results. (Easy.)
- [ ] **"More like this"** — open a recording → find semantically similar ones. Nearly free: search by an existing recording's stored vector instead of a fresh query embedding. (Promoted from "medium" — embeddings already exist.)
- [ ] **Saved searches / smart filters** — persist "meeting-tagged, last 30 days, contains 'action items'."

### ✨ Small wins

- [ ] **Auto-generated titles** — timestamped names don't scan. Ship the **first-line/keyword heuristic first** (no dependency); LLM-generated titles as an *optional* enhancement (requires a configured LLM + adds latency).
- [ ] **SRT / VTT export** — captions for a Loom/YouTube clip from an imported file.

---

## 📋 v1.10 — Local Intelligence

**Theme: make Recall a moat.** Bigger, model-touching work that builds on v1.9.

- [ ] **Transcript chunking + hybrid search** — embed per-passage (schema migration:
  the `embeddings` table is one-vector-per-recording today) and fuse FTS5 + vector
  (RRF). This is the *real* recall win — it's what makes "find the brief moment in a
  long recording" reliable. Add an in-memory embedding cache (today every query
  re-reads all BLOBs). *(Ideally add a CI job that can run the ONNX model.)*
- [ ] **"Ask my archive" (local RAG chat)** — "What did we decide about the API
  redesign?" → answer with citations/links to recordings. Builds on chunking +
  retrieval; needs a chat UI + citation UX. The headline differentiated feature.
- [ ] **Transcript ↔ waveform sync** — click a paragraph → seek playback. *(Needs
  word-level timestamps from v1.9.)*
- [ ] **Compare transcript versions** — side-by-side diff: original Whisper vs LLM
  cleanup vs manual edit. (`original_transcript` is already preserved.)
- [ ] **Custom vocabulary / glossary** — names like "Phoneme", "pyannote", client
  acronyms transcribed correctly via Whisper's `initial_prompt`. (Dictator persona,
  Whisper-native.)
- [ ] **Smart title + auto-tag suggestions** — after transcription, "Suggested tags:
  #meeting #design." (The LLM pipeline already runs optionally.)
- [ ] **Transcription queue dashboard** — pending / processing / failed with per-file
  error + retry, in the GUI.
- [ ] **Per-recording hook override** — this one goes to Discord, that one stays
  local. (Today hook config is global; re-fire is manual.)
- [ ] **Confidence highlighting** — low-confidence words underlined; click to fix.
  *(Needs word-level probabilities + segment storage — pairs with v1.9 word-level
  infra.)*

---

## 🔮 v2.0 — Platform & Integration

*Focus: cross-platform availability and opening Phoneme to external tools.*

### Platform
- [ ] **macOS port** — Apple Silicon first; bundled whisper.cpp server. Ship microphone-only first; do NOT let Meeting Mode block the macOS launch. `cpal` has no system-audio loopback on macOS — it requires a virtual device (BlackHole / Loopback). So on macOS: mic capture works natively; system-audio capture is opt-in via an external loopback device the user installs. Treat full feature parity as a follow-up, not a launch gate.
- [ ] **Linux port** — PipeWire / ALSA audio (PipeWire monitor sources give system-audio loopback natively, unlike macOS); X11 + Wayland global hotkey.
- [ ] **Windows ARM** — native ARM64 build for Snapdragon-based machines.

### Integration

> **Architecture decision (locked):** the daemon already speaks newline-delimited JSON over a named pipe behind the `phoneme-ipc` `Transport` trait. v2.0 adds an **HTTP front-end, not a new eventing model**: an `axum` server maps one-off `Request`s to REST endpoints (`POST /api/record/start`, `GET /api/recordings`) and streams `DaemonEvent`s as **Server-Sent Events** (`GET /api/events`, an `EventSource` in the frontend). REST API, browser extension, Raycast scripts, and the MCP server then all share one `fetch()`/`EventSource` surface.

- [ ] **Local REST API** — `localhost:3737` `axum` server (off by default): REST endpoints over the existing `Request`/`Response` enums + an SSE `/api/events` stream over `DaemonEvent`. Add an `HttpTransport` impl of the `Transport` trait so clients reuse the same typed surface.
- [ ] **MCP server** — `phoneme-mcp` binary (MCP = JSON-RPC over stdio). A **thin translator over the existing `Transport` trait**: `CallTool("start_recording")` maps to `Request::RecordStart` — near-zero business logic. Tools: `start_recording`, `stop_recording`, `get_transcript`, `search_recordings`, `list_recent`.
- [ ] **Webhook improvements** — HMAC-SHA256 signing; configurable trigger point (before hook, after hook, or independent); custom headers.
- [ ] **Browser extension** — toolbar icon; one click starts a recording and pastes the finished transcript into the focused field or clipboard. Requires the v2.0 REST API as the bridge.

### Recording
- [ ] **Multi-microphone** — capture from two input devices simultaneously (two-person interviews).
- [ ] **Audio normalization** — normalize gain before Whisper; improves accuracy on quiet voices.

### Data
- [ ] **Cloud sync** (opt-in, user-controlled) — encrypted sync of the catalog to a user-owned S3/Backblaze bucket; audio files excluded by default.

### Internal Quality
- [ ] **Playwright E2E UI coverage** — full E2E suite driving the frontend against the real Rust backend over IPC. After the architecture stabilizes across macOS/Linux.

---

## 🌌 Long Term

*No fixed timeline. Require significant platform work or community infrastructure.*

- [ ] **Mobile thin-client** — iOS/Android records locally, syncs to the desktop daemon over LAN; transcription runs on the desktop.
- [ ] **Plugin ecosystem** — standardized API for community hooks/themes/post-processors via a JSON registry.
- [ ] **Phoneme Cloud** (optional, self-hostable) — shared catalogs + role-based access for teams; the desktop daemon stays fully offline-capable.
- [ ] **Accessibility pass** — full NVDA/JAWS support, ARIA labels, font scaling, high-contrast theme.

### Architecture evolution
- [ ] **Protocol versioning** — version field on IPC Request/Response.
- [ ] **Batch operations** — batch delete / batch tag update.
- [ ] **Priority queue**, **parallel processing**, **health endpoint**, **metrics export**, **`--status` flag**.

### Data model
- [ ] **DB maintenance** (vacuum strategy), **indexing strategy** for 100k+ catalogs, **phrase search** (quoted FTS5).

### Security
- [ ] **Content Security Policy** (tauri.conf.json), **scoped permissions** (capabilities/default.json).

---

## 💰 Sustainability & Monetization

*Revenue ideas that keep the core desktop app 100% free, local, and open-source.*

- [ ] **Paid Mobile Companion App** — one-time fee / micro-subscription thin-client that records on the go and syncs back to the desktop daemon.
- [ ] **Phoneme Pro (Managed APIs)** — optional $8–10/mo for zero-config cloud Whisper + premium LLM cleanup without managing API keys.
- [ ] **Phoneme Sync** — $4–5/mo end-to-end-encrypted sync of the SQLite catalog + audio across machines (Obsidian-Sync-style).
- [ ] **Phoneme for Teams** — per-seat managed backend; centralized, searchable, role-governed meeting notes.

---

## ❌ Explicitly Not Doing

Considered and rejected — so we don't revisit them. (Speculative ideas that *might*
graduate someday live in the [Idea Parking Lot](docs/IDEAS.md) instead.)

| Idea | Reason |
|------|--------|
| Favorites / starring | Tags already do this — make a "⭐ Favorite" tag |
| Duration filter | Niche; nobody asked; search + tags already narrow the list |
| Backup/restore ZIP | Manual export covers it; the SQLite DB is a single copyable file |
| Azure Speech / AWS Transcribe | Enterprise pricing; not the target user; add only on demand |
| Portable (unsigned) ZIP | A CI task, not a product feature |
| Winget / Scoop packages | Same — packaging automation, not a roadmap item |
| Meeting-app awareness (auto-detect Zoom/Teams) | Brittle, false-positive-prone, and surveillance-y for a privacy-first app |
| Voice commands / wake word | Push-to-talk already solves hands-free; wake-word is a false-trigger rabbit hole |
| Transcript git-style version graph | YAGNI at this scale; original-vs-current diff covers ~95% |
| Acoustic echo cancellation (speaker→mic bleed) | Genuinely hard research problem; honest answer is "wear headphones" |
| Word-by-word streaming transcription | Moved to the v2.0 backlog; the bounded live preview covers the need for now |

---

## 🧰 Engineering & tech-debt backlog

*Not user-facing features — internal quality work, pulled in opportunistically
alongside the feature releases above.*

**Reliability**
- [ ] Retry/backoff for webhooks; rate limiting / circuit breakers for external services (OpenAI, Ollama, webhooks).
- [ ] Reconnection backoff/limit in `bridge.rs`.
- [ ] Replace remaining `unwrap()` in production paths (`recorder.rs` source opens; remaining hot paths).
- [ ] Integration tests for daemon components; a synthetic-audio E2E covering the single-recording path.

**Doctor**
- [ ] Disk-space + model-integrity checks; check categories (Critical/Warning/Info); per-check explanations + fix guidance; "Fix All"; Doctor in main nav (not just tray).

**Code organization**
- [ ] Split the large files (`config.rs`, `catalog.rs`, `recorder.rs`, `commands.rs`) into modules; dedupe `auto_spawn.rs` (CLI + Tauri); move `grouping.ts`/`form.ts` to `utils/`.
- [ ] Frontend: ESLint + Prettier; stricter TS (`noUnusedLocals`/`noUnusedParameters`); `types/` + `constants/` dirs.

**Performance**
- [ ] Trim redundant `http.clone()` (transcription.rs ×7, llm.rs ×4); avoid the `attention_mask` clone in `embed.rs`.

**Docs / DX**
- [ ] `config.example.toml` + `.env.example`; document JSON output + env vars; semantic-search + advanced-search-syntax docs; troubleshooting (audio devices, network/cloud, performance).
- [ ] Shell completions (bash/zsh/PowerShell); `cargo-audit`/`cargo-deny`; code coverage; consolidate `release_notes.md`/`.txt`.

**Testing & CI** *(from June 2026 audit — ~6/10 maturity; strong Rust foundation, integration gaps)*
- [x] **Gate `release.yml` on `cargo test` + vitest** — a `test` job (fmt + clippy + cargo test + vitest + type-check) now blocks the release job.
- [ ] **Pipeline integration tests** — the full transcribe → LLM → hooks → webhook → catalog/inbox path is the biggest untested critical path (`pipeline.rs`).
- [ ] **Webhook + embedding tests** — `webhook.rs` timeout/error contracts; embedding upsert/search round-trip + corrupt-BLOB handling.
- [ ] **Meeting capture E2E** (synthetic backend, incl. a paused-video internal gap) and **retention daemon** + **export-zip** integrity tests.
- [ ] Split CI: parallel `--lib`, serial `--test '*'` (today everything runs `--test-threads=1`); cache the `tauri-cli` binary.

**Quick correctness wins** *(small, mostly isolated)*
- [x] `embed.rs` mutex-poison `unwrap()` → `map_err` (no daemon panic on a poisoned lock). *(audit A-C2)*
- [x] Atomic `toggle_meeting()` — holds a `toggle_guard` across the read+act so concurrent toggles serialize. *(A-H11)*
- [x] Shared `Config::load_resolved()` so the CLI honors `PHONEME_CONFIG` like the daemon; shared `register_hotkeys(cfg)` so startup and a profile switch re-register *all* hotkeys (record/meeting/in-place). *(A-H13/A-H14)*
- [x] Structured Tauri IPC errors (`{ kind, message }`) instead of flattened strings; frontend reads them via `errText`/`errKind`. *(A-H6)*
- [x] Align `zip` versions across workspace vs tray (tray now uses the workspace `zip`). *(A-H12)*
- [x] Delete or implement the orphaned `checkMicrophoneAccess()` (no Tauri handler). *(A-C3)*
- [x] Consolidate the duplicated **Doctor** and triplicate **record-mode** enums into core. *(A-H3/A-H4)*

**Docs accuracy** *(audit found drift — fix the user-facing claims)*
- [ ] Say **speakrs**, not "Pyannote", everywhere (docs, `SectionDiarization.ts`). *(A-C5)*
- [ ] Reconcile claims that don't match code: `hook.log` (no writer), `phoneme config validate` (not a command — implement it, docs already claim it), inbox states (`processing/` also used), `HookPayload.original_transcript` (absent), "merged conversation = chronological" (currently stacked panes), semantic settings location (wizard-only), empty `docs/screenshots/`, Doctor hook-template path.

---

*Last reorganized around the four-persona model + a backend-ahead-of-GUI audit.
Pick a target version, decide whether "finish the attic" is one release or
background polish, and ship one medium feature (merged timeline or auto-titles)
per cycle.*
