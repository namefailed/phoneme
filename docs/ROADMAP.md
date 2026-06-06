# Phoneme Roadmap

This document tracks the full vision for Phoneme. Items are ordered by impact within each version.

**Design principle:** every item must pass the "would a real user hit this friction?" test. Features that duplicate existing functionality (e.g., "favorites" when tags exist) or serve fewer than ~10% of users are cut or moved to Long Term.

---

## ✅ v1.3.x — Maintenance (shipped)

- [x] Stale tag in filter dropdown after detach
- [x] Audit: shared format utilities, type-safe `UiFilter`, `RefireHook` config triple-load
- [x] Keyboard arrow-key navigation in the recordings list
- [x] Toast / snackbar notification system
- [x] Tray icon visual state change while recording is active
- [x] Whisper connectivity indicator + queue depth badge in the header bar
- [x] Window position and size persistence across restarts
- [x] Search term highlighting in transcript previews
- [x] Sort toggle on the recordings list (newest-first ↔ oldest-first)

---

## ✅ v1.4.0 — Polish & Power (shipped, test-verified)

- [x] **Cancel recording** button in the header bar with toast feedback
- [x] **Tag Manager** — rename tags, pick colors, delete from Settings
- [x] **Language selector** — pass BCP-47 language hint to Whisper; 20 languages + auto-detect
- [x] **Export** — single transcript (action row) and bulk catalog export (JSON / CSV / TXT)
- [x] **Auto-delete retention policy** — max age in days and/or max count; hourly daemon cleanup
- [x] **Extended hook presets** — grouped: Clipboard, Files, Obsidian, Discord webhook, Slack webhook, Python/Node scripts

---

## 🚀 v1.5.0 — Model Choice & Provider Flexibility

*The single biggest frustration for new users: they don't know which model to use, and the LLM settings require manually entering URLs and model names with no guidance. This version fixes that end-to-end.*

### Transcription — Multi-Provider Backend

Right now transcription is hardwired to whisper.cpp. A trait-based `TranscriptionProvider` abstraction lets users pick what runs their audio.

- [x] **OpenAI Whisper API** — cloud transcription via `api.openai.com/v1/audio/transcriptions`; just needs an API key; most accurate option for users without a local GPU
- [x] **Deepgram** — real-time-capable, good for long recordings; cheaper than OpenAI for bulk use
- [x] **AssemblyAI** — solid accuracy, built-in speaker diarization (who said what)
- [x] **Groq Whisper** — whisper-large-v3 via Groq's free tier; fastest cloud option today
- [x] **Provider picker in Settings → Whisper** — radio/select between: Local (whisper.cpp), OpenAI, Deepgram, AssemblyAI, Groq, Custom

> **Intentionally excluded:** Azure Speech, AWS Transcribe — too enterprise-focused; add only if users request them.

### Whisper Model Management

Users on low-end hardware get poor transcription not because Whisper is bad but because they're running the wrong model size.

- [x] **Model manager UI** — shows all GGML model variants (tiny·75 MB, base·142 MB, small·466 MB, medium·1.5 GB, large-v3·3.1 GB) with speed/accuracy tradeoffs written in plain English
- [x] **Hardware-aware recommendation** — detect available RAM (and GPU VRAM via DXGI on Windows) and auto-suggest the largest model that fits; surfaced as a tooltip/"Recommended" badge
- [x] **Per-model one-click download** — replace the single "Download Default" button with per-model download buttons; show progress and disk usage
- [x] **Re-transcribe with model picker** — action-row button that re-queues a recording against a different model; lets users upgrade quality on old recordings after switching to a bigger model

### LLM Post-Processing — Provider Flexibility

The current LLM settings are blank text boxes. Most users abandon them because they don't know what to type.

- [x] **Anthropic Claude** — `claude-3-haiku` and `claude-3-sonnet` via `api.anthropic.com`; add API key, select model, done
- [x] **Groq** — OpenAI-compatible; `llama-3.1-8b-instant` is free-tier and fast enough for cleanup
- [x] **LM Studio / OpenAI-compatible / Ollama** — generic "OpenAI-compatible endpoint" provider for LM Studio, Jan, text-generation-webui, Ollama, and any other local server
- [x] **Provider picker with live model list** — when a provider is selected and an API key entered, fetch available models and populate a dropdown (OpenAI, Anthropic, and Groq all have `/models` endpoints)
- [x] **Preset prompts** — saved library of named prompts (clean, summarize, extract action items, translate to English) rather than one editable text field; users can add their own
- [x] **Ollama setup wizard** — guided in-app flow that downloads and configures Ollama (not bundled in the installer); detects whether Ollama is already running, pulls the selected model, wires up the endpoint and model name automatically; users who already have Ollama just skip to the model-select step.

### UX
- [x] **Waveform visualization** — interactive waveform in the detail pane via wavesurfer.js: timeline, hover-seek, click-to-play, theme-aware colors
- [x] **Pause / resume recording** — ⏸ button during active recording; resumes without creating a new entry; essential for meeting notes
- [x] **Transcript history** — preserve the original Whisper output when a user manually edits; "View original" toggle + "Restore" button in the detail pane
- [x] **Word count & reading time** — "243 words · ~1 min read" in the detail footer; small scope, frequently useful
- [x] **Bulk actions** — Shift+Click and Ctrl+A to multi-select recordings; batch delete, re-transcribe, or export

### Data
- [x] **Custom date range filter** — date picker replacing the preset-only time dropdown
- [x] **Pre-deletion notification** — Windows toast before the retention cleanup runs: "3 recordings will be deleted in 24 hours per your retention policy"

---

## ✅ v1.6.0 — Real-time & Recording Quality (shipped & tagged)

*Focus: making the recording experience itself better — including full meeting capture.*

- [x] **Streaming transcription preview** — periodic re-transcription of the in-progress recording pushes a partial transcript to the UI in real time, so you're not staring at a "Transcribing…" wait (opt-in toggle)
- [x] **Windows loopback / system audio** — record from WASAPI loopback (speaker output) for transcribing meetings, videos, and any PC audio; foundation for Meeting Mode below
- [x] **Meeting Mode — dual-track capture** — simultaneously record microphone (your voice) and system audio (the meeting) as two separate streams; each is transcribed independently and stored as a linked pair under a shared session ID; use case: you get the meeting transcript *and* your own spoken notes/reactions as a separate document, both timestamped and searchable
- [x] **Session grouping in the recordings list** — linked recordings from a dual-track session appear as a collapsible group with a shared session label; expand to see the two tracks individually
- [x] **Pre-roll audio buffer** — rolling ring buffer so the first syllable isn't clipped when reacting to the hotkey (tunable; off by default)
- [x] **Notes field** — free-form text area in the detail pane, separate from the transcript; never overwritten by re-transcription or post-processing
- [x] **Multiple config profiles** — switch between named TOML profiles (e.g., work vs. personal) from the tray menu without editing files
- [x] **Import audio file** — bring a `.wav`/`.mp3`/`.m4a` into the catalog (or `phoneme import <file>`) to queue it through the same transcription + hook pipeline as a live recording

---

## ✅ v1.7.1 — Local Intelligence & Internal Quality (shipped)

*Focus: solidify the full Windows feature set — especially local, on-device AI —
and pay down internal debt, so the v2.0 cross-platform port inherits a complete,
clean base.*

### Local AI (on-device, offline)
- [x] **Local semantic search** — bundle a local embedding model (e.g. all-MiniLM-L6-v2 via ONNX) + a vector index so you can search by *meaning* ("that idea about rust error handling last week"), not just exact text. Complements the existing FTS5 keyword search.
- [x] **Merged conversation view** — interleave a dual-track meeting's two transcripts by timestamp into one chronological "You:" / "Meeting:" conversation; exportable, and feedable to the LLM post-processor as a single context for summaries/action items. **Build this on Lit (below), not raw `innerHTML`** — interleaving two dynamic arrays while preserving interactive elements (play/edit state) is exactly the case manual DOM templating handles badly.

### Internal quality
- [x] **Frontend reactivity (Lit for complex views)** — the framework-less `Store.ts` pattern is great for flat lists/forms and stays. But adopt **Lit (Web Components)** for the complex, dynamically-reconciled views (the merged conversation timeline first) to get declarative rendering + automatic lifecycle/listener cleanup without a full React/Vue. Do this *before* the merged conversation view.
- [x] **Test audio backend for full CI E2E** — the `Source` trait already abstracts capture (`CpalSource` prod, `SyntheticSource` tests), and Meeting Mode is end-to-end testable via `start_meeting_with_sources`. Extend the same injection to the **single-recording** daemon path so a CI test can drive CLI → daemon → (mock sine/silence) capture → SQLite without hardware, closing the "cpal device tests skipped in CI" gap.
- [x] **Typed errors** — `thiserror` for the library crates, `anyhow` in the binaries, for clean `?` propagation and better traces.
- [x] **Paginated recordings list** — `ListFilter` has `limit` but no `offset`, and the GUI fetches the list unpaginated. At 5,000+ recordings that floods the named pipe and hydrates thousands of `RecordingsList` rows at once, locking the UI thread and ballooning memory. Add `offset` to `ListRecordings` + catalog `list()`, plus a "Load More" / `IntersectionObserver` infinite scroll in `RecordingsList.ts`. (Pairs with the Lit adoption above.)

---

## ✅ v1.7.5 — Advanced Streaming & Diarization (shipped)

*Focus: Completion of the v1.7.x milestone — CI quality, UX polish, and internal hardening.*

- [x] **Synthetic audio CI backend** — full end-to-end CI test coverage via a `GeneratorSource` mock; drives CLI → daemon → capture → SQLite without hardware; closes the \"cpal device tests skipped in CI\" gap from v1.7.1.
- [x] **Meeting session indentation in recordings list** — expanded meeting groups visually indent their child tracks so standalone recordings are never confused with session members.
- [x] **rustfmt / CI hygiene** — formatter enforced on all modified files; all branches merged to master; `v1.7.5` tagged clean.
- [ ] **Local speaker diarization** — label Speaker A / Speaker B in Meeting Mode transcripts using a local diarization model alongside Whisper (moved to v2.0 backlog).
- [ ] **Real-time word-by-word transcription** — upgrade the v1.6 streaming *preview* to true word-level streaming as you speak (moved to v2.0 backlog).

---

## 🔮 v2.0 — Platform & Integration

*Focus: cross-platform availability and opening Phoneme to external tools.*

### Platform
- [ ] **macOS port** — Apple Silicon first; bundled whisper.cpp server. **Ship microphone-only first; do NOT let Meeting Mode block the macOS launch.** `cpal` has no system-audio loopback on macOS — it requires a virtual device (BlackHole / Loopback). So on macOS: mic capture works natively; system-audio capture is opt-in via an external loopback device the user installs. Treat full feature parity as a follow-up, not a launch gate.
- [ ] **Linux port** — PipeWire / ALSA audio (PipeWire monitor sources give system-audio loopback natively, unlike macOS); X11 + Wayland global hotkey
- [ ] **Windows ARM** — native ARM64 build for Snapdragon-based machines

### Integration

> **Architecture decision (locked):** the daemon already speaks newline-delimited
> JSON over a named pipe behind the `phoneme-ipc` `Transport` trait. v2.0 adds an
> **HTTP front-end, not a new eventing model**: an `axum` server maps one-off
> `Request`s to REST endpoints (`POST /api/record/start`, `GET /api/recordings`)
> and streams `DaemonEvent`s as **Server-Sent Events** (`GET /api/events`, an
> `EventSource` in the frontend). REST API, browser extension, Raycast scripts,
> and the MCP server then all share one `fetch()`/`EventSource` surface.

- [ ] **Local REST API** — `localhost:3737` `axum` server (off by default): REST endpoints over the existing `Request`/`Response` enums + an SSE `/api/events` stream over `DaemonEvent`. Add an `HttpTransport` impl of the `Transport` trait so clients reuse the same typed surface.
- [ ] **MCP server** — `phoneme-mcp` binary (MCP = JSON-RPC over stdio). Implement it as a **thin translator over the existing `Transport` trait**: a `CallTool("start_recording")` just maps to `Request::RecordStart` and fires it at the daemon over the pipe/socket — near-zero business logic in the MCP crate. Exposes tools: `start_recording`, `stop_recording`, `get_transcript`, `search_recordings`, `list_recent`.
- [ ] **Webhook improvements** — HMAC-SHA256 signing; configurable trigger point (before hook, after hook, or independent); custom headers
- [ ] **Browser extension** — Chrome/Firefox extension that adds a Phoneme icon to the toolbar; one click starts a recording and pastes the finished transcript into the focused input field or copies it to the clipboard; requires the v2.0 local REST API as the bridge

### Recording
- [ ] **Multi-microphone** — capture from two input devices simultaneously; useful for two-person interviews
- [ ] **Audio normalization** — normalize gain before sending to Whisper; improves accuracy on quiet voices

### Data
- [ ] **Cloud sync** (opt-in, user-controlled) — encrypted sync of the catalog to a user-owned S3/Backblaze bucket for multi-machine access; audio files excluded by default

---

## 🌌 Long Term
*No fixed timeline. These require either significant platform work or community infrastructure.*

- [ ] **Mobile thin-client** — iOS/Android app that records locally and syncs to the desktop daemon over LAN; transcription runs on the desktop
- [ ] **Plugin ecosystem** — standardized API for community hooks, themes, and post-processors; distributed via a JSON registry
- [ ] **Phoneme Cloud** (optional, self-hostable) — shared catalogs and role-based access for teams; the desktop daemon remains fully offline-capable
- [ ] **Accessibility pass** — full NVDA/JAWS screen reader support, ARIA labels, font-size scaling, high-contrast theme

---

## ❌ Explicitly Not Doing

Things that were considered and rejected — so we don't revisit them:

| Idea | Reason |
|------|--------|
| Favorites / starring | Tags already do this — create a "⭐ Favorite" tag |
| Duration filter | Niche; no user has asked; search + tags already narrow the list |
| Backup/restore ZIP | Manual export covers this; SQLite DB is already a single copyable file |
| Azure Speech / AWS Transcribe | Enterprise pricing; not the Phoneme target user; add if demand emerges |
| Portable (unsigned) ZIP | Valid distribution target but a CI task, not a product feature; just ship it |
| Winget / Scoop packages | Same — automation task for when v1.5 ships, not a roadmap feature |
