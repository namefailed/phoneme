# 🗺️ Phoneme Roadmap

This roadmap tracks planned features, improvements, and strategic initiatives for Phoneme.

---

## 🚀 Immediate (This Week)

**High-impact, low-effort fixes and improvements**

### Critical Fixes
- [ ] Fix broken documentation references (docs/INTERNAL.md → internals.md in CONTRIBUTING.md)
- [ ] Add error handling to to-clipboard.ps1 (try-catch for clipboard operations)
- [ ] Add ESLint to frontend (package.json lint script)
- [ ] Add Prettier to frontend (package.json format script)

### High-Value Quick Wins
- [ ] Add loading spinner to App.ts (show loading state during initial config load)
- [ ] Add shell completion for CLI (bash, zsh, PowerShell completions)
- [ ] Add progress reporting for transcriptions (progress callback for long operations)
- [ ] Add reconnection backoff/limit to bridge.rs (exponential backoff, max attempts)

### Doctor Improvements
- [ ] Add disk space check to doctor (check available disk before recording)
- [ ] Add model validation to doctor (validate model file existence and integrity)
- [ ] Add Doctor to main navigation (not just tray icon right-click)
- [ ] Add explanations for each check (what it means, why it matters)
- [ ] Add detailed error messages with context and guidance
- [ ] Add check categories (Critical, Warning, Info) with visual indicators
- [ ] Add step-by-step fix guidance for failed checks
- [ ] Add "Fix All" button for common auto-fixable issues
- [ ] Add links to documentation for each check

---

## 📋 v1.8.x (Next Sprint)

**CLI Enhancements**
- [ ] Add --config flag (override config file path via CLI)
- [ ] Add --validate command (check config without applying)
- [ ] Add --dry-run flag (preview config changes)

**Frontend Polish**
- [ ] Add keyboard shortcuts (Ctrl+, for settings, Ctrl+A select all, Esc clear)
- [ ] Add "Reset to Defaults" button per section (SettingsView)
- [ ] Add config import for backup (SettingsView) - export already exists
- [ ] Add ETA calculation for model downloads (FirstRunWizard) - progress exists
- [ ] Add drag-drop visual feedback (import.ts) - handler exists
- [ ] Add batch import progress (show total count for multiple files)

**Backend Reliability**
- [ ] Add retry logic with exponential backoff for webhooks
- [ ] Add token tracking for paid APIs (monitor costs)
- [ ] Add batch embedding API (efficiency)
- [ ] Add embedding cache (cache for repeated texts)

**Tauri UX**
- [ ] Add splash screen during daemon spawn (progress indicator)
- [ ] Add spawn cancellation (cancel if user closes app during startup)
- [ ] Add Ollama test to wizard (endpoint test)
- [ ] Add reconnection status events (emit status for UI feedback)

**Documentation**
- [ ] Add System Requirements to README (Windows version, RAM requirements)
- [ ] Add Roadmap section to README (communicate future direction)
- [ ] Document CLI exit codes (for scripting)
- [ ] Document JSON output format (examples)
- [ ] Document environment variables (reference)

**Code Quality & Organization**
- [ ] Standardize binary command file naming (remove _cmd suffix or add to all)
- [ ] Rename summarize-with-ollama.ps1 to to-ollama-summary.ps1 for consistency
- [ ] Standardize script naming (gen- vs generate- prefix)
- [ ] Deduplicate auto_spawn.rs (share between CLI and Tauri)
- [ ] Move queue_worker_test.rs to tests/ directory
- [ ] Split config.rs into config/ subdirectory (54KB file)
- [ ] Split catalog.rs into catalog/ subdirectory (38KB file)
- [ ] Split recorder.rs into recorder/ subdirectory (51KB file)
- [ ] Split commands.rs into commands/ subdirectory (41KB file)
- [ ] Move grouping.ts from RecordingsView/ to utils/ or state/
- [ ] Move form.ts from SettingsView/ to utils/
- [ ] Remove or document replace_session.py at root
- [ ] Consolidate release_notes.md and release_notes.txt (choose canonical)
- [ ] Document or remove scratch/ directory
- [ ] Document or remove archive_internal/ directory
- [ ] Add types/ directory to frontend for TypeScript type definitions
- [ ] Add constants/ directory to frontend for constants
- [ ] Add .env.example file to document environment variables
- [ ] Add config.example.toml to document configuration options

**Security & Reliability**
- [ ] Verify innerHTML usage safety (SectionWhisper.ts provider metadata)
- [ ] Add rate limiting for HTTP clients (prevent API overwhelm)
- [ ] Add circuit breaker for external services (OpenAI, Ollama, webhooks)
- [ ] Add request timeout configuration for all HTTP clients
- [ ] Add structured logging correlation IDs for request tracing
- [ ] Add health check endpoint for daemon monitoring
- [ ] Add graceful shutdown for in-progress operations

**Performance**
- [ ] Reduce unnecessary clones in transcription.rs (http.clone() called 7 times)
- [ ] Reduce unnecessary clones in llm.rs and embed.rs (use Arc/borrowing)

**Code Quality**
- [ ] Replace unwrap() in production code (whisper_supervisor.rs, pipeline.rs, doctor.rs)
- [ ] Add integration tests for daemon components (recorder, pipeline, ipc_handler)

---

## 📋 v1.9.x (Following Sprint)

**Frontend UX**
- [ ] Add folder import option (import.ts)
- [ ] Add error toast for failed theme application (App.ts)

**Backend Enhancements**
- [ ] Add streaming LLM responses (real-time feedback)
- [ ] Add webhook authentication (Bearer token/API key support)
- [ ] Make diarization configurable (GPU support, configurable step/duration)
- [ ] Add batch embedding API for efficiency
- [ ] Add embedding cache for repeated texts

**Security**
- [ ] Add IPC rate limiting (prevent abuse)
- [ ] Add IPC metrics/logging (request logging for debugging)
- [ ] Add request size limits (prevent abuse)

**Documentation**
- [ ] Add Python IPC example (integration guide)
- [ ] Document error handling patterns for IPC
- [ ] Document reconnection strategy for IPC
- [ ] Add semantic search documentation
- [ ] Document advanced search syntax (quotes, operators)
- [ ] Add keyboard shortcuts reference
- [ ] Add performance troubleshooting section
- [ ] Add audio device troubleshooting
- [ ] Add network/cloud API troubleshooting

**Hooks UX**
- [ ] Add character limit warning for large transcripts (to-clipboard.ps1)
- [ ] Add file rotation for large files (to-file.ps1)
- [ ] Add timeout configuration for webhooks (to-webhook.ps1)

---

## 🔮 v2.0 — Platform & Integration

*Focus: cross-platform availability and opening Phoneme to external tools.*

### Platform
- [ ] **macOS port** — Apple Silicon first; bundled whisper.cpp server. Ship microphone-only first; do NOT let Meeting Mode block the macOS launch. `cpal` has no system-audio loopback on macOS — it requires a virtual device (BlackHole / Loopback). So on macOS: mic capture works natively; system-audio capture is opt-in via an external loopback device the user installs. Treat full feature parity as a follow-up, not a launch gate.
- [ ] **Linux port** — PipeWire / ALSA audio (PipeWire monitor sources give system-audio loopback natively, unlike macOS); X11 + Wayland global hotkey
- [ ] **Windows ARM** — native ARM64 build for Snapdragon-based machines

### Integration

> **Architecture decision (locked):** the daemon already speaks newline-delimited JSON over a named pipe behind the `phoneme-ipc` `Transport` trait. v2.0 adds an **HTTP front-end, not a new eventing model**: an `axum` server maps one-off `Request`s to REST endpoints (`POST /api/record/start`, `GET /api/recordings`) and streams `DaemonEvent`s as **Server-Sent Events** (`GET /api/events`, an `EventSource` in the frontend). REST API, browser extension, Raycast scripts, and the MCP server then all share one `fetch()`/`EventSource` surface.

- [ ] **Local REST API** — `localhost:3737` `axum` server (off by default): REST endpoints over the existing `Request`/`Response` enums + an SSE `/api/events` stream over `DaemonEvent`. Add an `HttpTransport` impl of the `Transport` trait so clients reuse the same typed surface.
- [ ] **MCP server** — `phoneme-mcp` binary (MCP = JSON-RPC over stdio). Implement it as a **thin translator over the existing `Transport` trait**: a `CallTool("start_recording")` just maps to `Request::RecordStart` and fires it at the daemon over the pipe/socket — near-zero business logic in the MCP crate. Exposes tools: `start_recording`, `stop_recording`, `get_transcript`, `search_recordings`, `list_recent`.
- [ ] **Webhook improvements** — HMAC-SHA256 signing; configurable trigger point (before hook, after hook, or independent); custom headers
- [ ] **Browser extension** — Chrome/Firefox extension that adds a Phoneme icon to the toolbar; one click starts a recording and pastes the finished transcript into the focused input field or copies it to the clipboard; requires the v2.0 local REST API as the bridge

### Recording
- [ ] **Multi-microphone** — capture from two input devices simultaneously; useful for two-person interviews
- [ ] **Audio normalization** — normalize gain before sending to Whisper; improves accuracy on quiet voices

### Data
- [ ] **Cloud sync** (opt-in, user-controlled) — encrypted sync of the catalog to a user-owned S3/Backblaze bucket for multi-machine access; audio files excluded by default

### Internal Quality
- [ ] **Playwright E2E UI Coverage** — add a full End-to-End test suite using Playwright (or Tauri WebDriver) to interact with the frontend UI and exercise the actual Rust backend via IPC. To be implemented *after* the architecture stabilizes across macOS and Linux.

---

## 🌌 Long Term

*No fixed timeline. These require either significant platform work or community infrastructure.*

- [ ] **Mobile thin-client** — iOS/Android app that records locally and syncs to the desktop daemon over LAN; transcription runs on the desktop
- [ ] **Plugin ecosystem** — standardized API for community hooks, themes, and post-processors; distributed via a JSON registry
- [ ] **Phoneme Cloud** (optional, self-hostable) — shared catalogs and role-based access for teams; the desktop daemon remains fully offline-capable
- [ ] **Accessibility pass** — full NVDA/JAWS screen reader support, ARIA labels, font-size scaling, high-contrast theme

### Architecture Evolution
- [ ] **Protocol versioning** — Add version field to IPC Request/Response for future evolution
- [ ] **Batch operations** — Implement batch delete, batch update tags for efficiency
- [ ] **Priority queue** — Implement priority queue for urgent recordings
- [ ] **Parallel processing** — Add configurable parallel processing for faster throughput
- [ ] **Health endpoint** — Add HTTP /health endpoint for daemon monitoring
- [ ] **Metrics export** — Add Prometheus metrics for monitoring performance
- [ ] **Status command** — Add --status flag to show recording state without starting

### Data Model
- [ ] **Database maintenance** — Implement vacuum and maintenance strategy for long-running databases
- [ ] **Indexing strategy** — Document indexing strategy for large catalogs (100k+ recordings)
- [ ] **Phrase search** — Enhance FTS5 query sanitization to support quoted phrase search

### Security
- [ ] **Content Security Policy** — Add CSP for better security (tauri.conf.json)
- [ ] **Scoped permissions** — Replace broad default permissions with scoped permissions (capabilities/default.json)

### Developer Experience
- [ ] **TypeScript path aliases** — Add @/components, etc. for cleaner imports (tsconfig.json)
- [ ] **Stricter TypeScript** — Enable noUnusedLocals and noUnusedParameters
- [ ] **Incremental compilation** — Enable for faster builds (tsconfig.json)
- [ ] **Bundle analyzer** — Add for size monitoring (vite.config.ts)
- [ ] **Code coverage** — Implement with codecov
- [ ] **Dependency scanning** — Add cargo-audit or cargo-deny for security
- [ ] **Cross-platform CI** — Add Linux/macOS CI for validation
- [ ] **Changelog generation** — Automate from commits
- [ ] **Release notes** — Generate from conventional commits

---

## 💰 Sustainability & Monetization

*Ideas for generating revenue while keeping the core desktop app 100% free, local, and open-source.*

- [ ] **Paid Mobile Companion App** — a one-time fee (or micro-subscription) thin-client for iOS/Android that records audio on the go and syncs it securely back to the desktop daemon for processing.
- [ ] **Phoneme Pro (Managed APIs)** — an optional $8-$10/mo subscription where users get instant, zero-config access to ultra-fast cloud Whisper transcription and premium LLM Smart Cleanup (e.g., Claude 3.5 Sonnet) without needing to manage their own developer accounts or API keys.
- [ ] **Phoneme Sync** — a low-cost ($4-$5/mo) end-to-end encrypted cloud sync service for power users who want their SQLite catalog and audio files seamlessly synchronized across multiple machines (similar to Obsidian Sync).
- [ ] **Phoneme for Teams** — a managed, per-seat enterprise backend where dual-track meeting notes are centralized, searchable by the whole team, and strictly governed by role-based access.

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
