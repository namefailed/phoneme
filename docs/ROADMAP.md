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

- [ ] **OpenAI Whisper API** — cloud transcription via `api.openai.com/v1/audio/transcriptions`; just needs an API key; most accurate option for users without a local GPU
- [ ] **Deepgram** — real-time-capable, good for long recordings; cheaper than OpenAI for bulk use
- [ ] **AssemblyAI** — solid accuracy, built-in speaker diarization (who said what)
- [ ] **Groq Whisper** — whisper-large-v3 via Groq's free tier; fastest cloud option today
- [ ] **Provider picker in Settings → Whisper** — radio/select between: Local (whisper.cpp), OpenAI, Deepgram, AssemblyAI, Groq

> **Intentionally excluded:** Azure Speech, AWS Transcribe — too enterprise-focused; add only if users request them.

### Whisper Model Management

Users on low-end hardware get poor transcription not because Whisper is bad but because they're running the wrong model size.

- [ ] **Model manager UI** — shows all GGML model variants (tiny·75 MB, base·142 MB, small·466 MB, medium·1.5 GB, large-v3·3.1 GB) with speed/accuracy tradeoffs written in plain English
- [ ] **Hardware-aware recommendation** — detect available RAM (and GPU VRAM via DXGI on Windows) and auto-suggest the largest model that fits; surfaced as a tooltip/"Recommended" badge
- [ ] **Per-model one-click download** — replace the single "Download Default" button with per-model download buttons; show progress and disk usage
- [ ] **Re-transcribe with model picker** — action-row button that re-queues a recording against a different model; lets users upgrade quality on old recordings after switching to a bigger model

### LLM Post-Processing — Provider Flexibility

The current LLM settings are blank text boxes. Most users abandon them because they don't know what to type.

- [ ] **Anthropic Claude** — `claude-3-haiku` and `claude-3-sonnet` via `api.anthropic.com`; add API key, select model, done
- [ ] **Groq** — OpenAI-compatible; `llama-3.1-8b-instant` is free-tier and fast enough for cleanup
- [ ] **LM Studio / OpenAI-compatible** — generic "OpenAI-compatible endpoint" provider for LM Studio, Jan, text-generation-webui, and any other local server
- [ ] **Provider picker with live model list** — when a provider is selected and an API key entered, fetch available models and populate a dropdown (OpenAI, Anthropic, and Groq all have `/models` endpoints)
- [ ] **Preset prompts** — saved library of named prompts (clean, summarize, extract action items, translate to English) rather than one editable text field; users can add their own
- [ ] **Ollama setup wizard** — guided in-app flow that downloads and configures Ollama (not bundled in the installer); detects whether Ollama is already running, pulls the selected model, wires up the endpoint and model name automatically; users who already have Ollama just skip to the model-select step

### UX
- [ ] **Waveform visualization** — render the actual audio waveform in the detail pane canvas element using the Web Audio API; the placeholder is already in the HTML
- [ ] **Pause / resume recording** — ⏸ button during active recording; resumes without creating a new entry; essential for meeting notes
- [ ] **Transcript history** — preserve the original Whisper output when a user manually edits; "View original" toggle + "Restore" button in the detail pane
- [ ] **Word count & reading time** — "243 words · ~1 min read" in the detail footer; small scope, frequently useful
- [ ] **Bulk actions** — Shift+Click and Ctrl+A to multi-select recordings; batch delete, re-transcribe, or export

### Data
- [ ] **Custom date range filter** — date picker replacing the preset-only time dropdown
- [ ] **Pre-deletion notification** — Windows toast before the retention cleanup runs: "3 recordings will be deleted in 24 hours per your retention policy"

---

## 🔮 v1.6.0 — Real-time & Recording Quality

*Focus: making the recording experience itself better.*

- [ ] **Streaming transcription preview** — use whisper.cpp's streaming endpoint to push partial transcript tokens to the UI in real time; eliminates the "Transcribing…" wait
- [ ] **Windows loopback / system audio** — record from WASAPI loopback (speaker output) for transcribing meetings, videos, and any PC audio; add as a second input source option
- [ ] **Pre-roll audio buffer** — 500 ms ring buffer so the first syllable isn't clipped when reacting to the hotkey
- [ ] **Notes field** — free-form text area in the detail pane, separate from the transcript; never overwritten by re-transcription or AI
- [ ] **Multiple config profiles** — switch between named TOML profiles (e.g., work vs. personal) from the tray menu without editing files
- [ ] **Import audio file** — drag a `.wav`/`.mp3`/`.m4a` onto the app window (or `phoneme import <file>`) to queue it for transcription

---

## 🔮 v2.0 — Platform & Integration

*Focus: cross-platform availability and opening Phoneme to external tools.*

### Platform
- [ ] **macOS port** — Apple Silicon first; bundled whisper.cpp server; full feature parity with Windows
- [ ] **Linux port** — PipeWire / ALSA audio; X11 + Wayland global hotkey
- [ ] **Windows ARM** — native ARM64 build for Snapdragon-based machines

### Integration
- [ ] **Local REST API** — `localhost:3737` HTTP server (off by default) exposing list, get, and event-stream endpoints; enables Obsidian plugins, Raycast extensions, and shell scripts
- [ ] **Webhook improvements** — HMAC-SHA256 signing; configurable trigger point (before hook, after hook, or independent); custom headers
- [ ] **Browser extension** — Chrome/Firefox extension that adds a Phoneme icon to the toolbar; one click starts a recording and pastes the finished transcript into the focused input field or copies it to the clipboard; requires the v2.0 local REST API as the bridge

### Recording
- [ ] **Real-time word-by-word transcription** — live transcript appears as you speak using `whisper-live` or a streaming-capable backend; requires v1.6 streaming foundation
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
