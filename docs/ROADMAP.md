# Phoneme Roadmap

This document tracks the full vision for Phoneme — from near-term polish through long-term platform expansion. Items within each version are roughly prioritized top-to-bottom.

---

## ✅ v1.3.x — Maintenance (current)
*Ongoing patch releases: bug fixes and small improvements that don't require a feature flag.*

- [x] Stale tag in filter dropdown after detach — fixed in v1.3.1
- [x] Audit: shared format utilities, type-safe `UiFilter`, `RefireHook` config triple-load, inline style extraction — v1.3.1
- [ ] Keyboard arrow-key navigation in the recordings list (Up/Down to move, Enter to open)
- [ ] Toast / snackbar notification system — non-blocking feedback for copy, delete, export
- [ ] Tray icon visual state change while recording is active
- [ ] Whisper connectivity indicator in the header bar (reacts to `WhisperStatusChanged` events)
- [ ] Queue depth badge in the header (reacts to `QueueDepthChanged` events)
- [ ] Window position and size persistence across restarts
- [ ] Search term highlighting in transcript previews
- [ ] Sort toggle on the recordings list (newest-first ↔ oldest-first)

---

## 🚀 v1.4 — Polish & Power
*Focus: filling the gaps that power users hit every day.*

### UX & Interface
- [ ] **Tag Manager** — dedicated settings panel to rename tags, pick colors with a proper color picker, reorder, and bulk-delete
- [ ] **Custom date range filter** — replace the preset-only time dropdown with an actual date picker
- [ ] **Duration filter** — filter recordings by minimum/maximum length
- [ ] **Bulk actions** — multi-select recordings (Shift+Click, Ctrl+A) for batch delete, re-transcribe, or export
- [ ] **Column sorting** — click column headers to sort by date, duration, or status
- [ ] **Favorites / starring** — star important recordings; add a "starred" filter
- [ ] **Cancel button** during active recording (the IPC `RecordCancel` command already exists; just wire the UI)

### Recording Quality
- [ ] **Pre-roll audio buffer** (~500 ms ring buffer) so the first word isn't clipped when reacting to the hotkey
- [ ] **Language selector** — expose Whisper's `--language` parameter as a per-session or global setting (Whisper supports 97 languages)
- [ ] **Audio device hot-switch recovery** — detect device disconnect mid-recording and fall back gracefully instead of silently dropping audio

### AI / LLM
- [ ] **User-defined prompt library** — replace the 9 hardcoded presets with a saved, editable list of custom prompts
- [ ] **Per-recording AI post-processing** — "Run AI on this transcript" button in the detail pane, with prompt picker
- [ ] **Chained post-processing** — define an ordered list of LLM passes (e.g., clean → summarize → extract action items) that run sequentially

### Data Management
- [ ] **Export** — export selected recordings or entire catalog as JSON, CSV, or plain-text TXT
- [ ] **Import audio** — drag an audio file onto the app (or `phoneme import <file.wav>`) to queue it for transcription
- [ ] **Auto-delete / retention policy** — configurable rule: "delete recordings older than N days" or "keep only last N recordings"
- [ ] **Backup / restore** — one-click export of the SQLite catalog + audio files as a zip archive

### Distribution
- [ ] **Bundled Ollama** — ship Ollama binaries with the installer for fully offline AI without manual setup
- [ ] **Portable build** — unsigned ZIP alongside the MSI for users who can't run installers
- [ ] **Winget package** — submit to the Windows Package Manager community repo
- [ ] **Scoop package** — add a Scoop bucket entry
- [ ] **Extended hook presets** — Notion, Obsidian vault drop, Discord webhook, Slack webhook, email via SMTP

### macOS
- [ ] **macOS Beta** — early Apple Silicon port; feature parity for recording + transcription + hooks; no bundled whisper-server yet

---

## 🔮 v1.5 — Intelligence & Integration
*Focus: making Phoneme smarter about what it captures and easier to integrate with everything else.*

### AI
- [ ] **Summary field** — auto-generated one-sentence summary stored alongside each transcript; shown in the list as an optional column
- [ ] **Action item extraction** — dedicated LLM pass that produces a structured list of action items, stored per recording and copyable independently
- [ ] **Semantic search** — local embedding index (e.g., via Ollama) enabling "find recordings similar to this phrase" beyond FTS keyword matching

### Integration
- [ ] **Local REST API** — expose an HTTP API (localhost only) so any external app, script, or browser extension can subscribe to events, list recordings, or trigger recording
- [ ] **Browser extension** — trigger recording from the browser; paste transcript into the focused input field
- [ ] **Obsidian / Logseq hook preset** — built-in preset that appends transcripts to a daily note in a configured vault path
- [ ] **Webhook improvements** — fire webhook before hook, after hook, or independently; support custom headers and auth tokens

### Data
- [ ] **Transcript history** — store previous versions of a transcript when it is manually edited or re-processed, with a diff view and one-click restore
- [ ] **Notes field** — a free-form notes area separate from the transcript, not touched by AI or re-transcription
- [ ] **Multiple profiles** — switch between named config profiles (e.g., work vs. personal) without editing TOML manually

### Platform
- [ ] **macOS full port** — Intel + Apple Silicon; bundled whisper-server; full feature parity with Windows
- [ ] **Linux port** — PipeWire / ALSA audio; X11 + Wayland global hotkey via `evdev` or `rdev`
- [ ] **Windows ARM** — native ARM64 build for Snapdragon-based Windows machines

---

## 🔮 v2.0 — Platform & Real-time
*Focus: streaming and cross-platform maturity.*

- [ ] **Streaming transcription** — live transcript appears word-by-word as you speak using Whisper streaming / `whisper-live`; no waiting for recording to stop
- [ ] **Multi-microphone** — capture from multiple input devices simultaneously (e.g., headset + room mic), merge or keep separate
- [ ] **Noise suppression** — optional pre-processing pass (RNNoise or similar) before sending audio to Whisper
- [ ] **Audio normalization** — normalize gain before transcription for better accuracy on quiet voices
- [ ] **Cloud sync** (opt-in) — encrypted sync of the catalog (not audio) to a user-controlled S3/Backblaze bucket for multi-machine access

---

## 🌌 Long Term Vision
*No fixed timeline — ideas that require either significant infrastructure or platform work.*

- [ ] **Mobile thin-client** — iOS/Android companion that records locally and streams to the desktop daemon over LAN or a self-hosted relay; transcription and hooks run on the desktop
- [ ] **Plugin ecosystem** — a standardized plugin API for community-contributed hooks, themes, and post-processors; distributed via a simple JSON registry
- [ ] **Accessibility pass** — full screen reader (NVDA/JAWS) support, ARIA labels, font-size scaling, high-contrast theme, `prefers-reduced-motion` handling
- [ ] **Phoneme Cloud** (optional, self-hostable) — a lightweight server component for teams: shared catalogs, role-based access, audit log; the desktop daemon remains fully functional offline
