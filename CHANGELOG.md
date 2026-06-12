# 📦 Phoneme Changelog

Shipped releases — what landed in each. **Forward-looking plans live in [`ROADMAP.md`](ROADMAP.md)**; unvetted/parked ideas live in [`docs/IDEAS.md`](docs/IDEAS.md).

---

## 🚧 v1.8.x — Recall, Meetings & Hardening (in development)

*Workspace version `1.8.1`. Closing promise-vs-reality gaps and hardening the
trust boundary. Verified against current code.*

### Recall

- [x] **Chunked hybrid semantic search** — transcripts are split into overlapping,
  sentence-aware chunks (`phoneme-core::chunk`), each embedded into a new
  `embedding_chunks` table; a recording is scored by its best-matching chunk. The
  vector ranking is fused with FTS5 via Reciprocal Rank Fusion (`fusion.rs`,
  `catalog::hybrid_search`), and cosine is calibrated to a 0–100% relevance chip.
  Big paraphrase-recall win on longer notes.
- [x] **Embedding model as a user choice** — `[semantic_search]` gained `max_tokens`,
  `pooling`, `token_type_ids`, and `query_prefix` / `passage_prefix`, so E5/BGE-class
  models work alongside the bundled all-MiniLM-L6-v2. A dedicated **Semantic Search**
  settings section exposes them, plus a **Re-embed all recordings** action
  (`ReembedAll` IPC) that re-indexes the library after a model change.
- [x] **Semantic relevance chip** in the recordings list during a semantic query.

### Meetings

- [x] **Merged meeting view** — selecting a meeting's group header opens a single,
  read-only reading of every track, labelled 🎤 Microphone / 🔊 System audio with the
  diarizer's `[Speaker N]` turns surfaced, plus Copy / Export
  (`MergedConversationDetail.ts`, `mergeMeeting.ts`). Coarse/source-sectioned — not
  yet chronologically interleaved.

### Recording

- [x] **System-wide live-preview overlay** — an opt-in, always-on-top, frameless
  desktop window that floats the live caption over any app, even when the main
  window is hidden (`src-tauri/src/overlay.rs`, `frontend/overlay.*`); gated on
  `interface.preview_overlay`. Off by default.

### Security & reliability

- [x] **Masked config at the WebView boundary (S-H2)** — API keys are masked before
  `read_config` reaches the renderer and restored from disk on save, so secrets
  never leave the daemon side (`src-tauri/src/commands.rs`).
- [x] **IPC connection resilience** — an unknown or unparseable request returns an
  error `Response` and keeps the pipe open instead of tearing down the connection
  (`ServerRequest::Unknown`, `phoneme-ipc`).

- [x] **Baseline CSP + narrowed scopes (S-H4/S-H6)** — real production CSP (scripts
  locked to the bundle; connect-src open only because Settings fetches provider
  model lists at user-configured endpoints), devCsp for vite, asset protocol
  narrowed from the whole home directory to the audio + app-data dirs, unused
  window capabilities dropped (`tauri.conf.json`, `capabilities/default.json`).
- [x] **Doctor: categories, disk + model-integrity checks, Fix All** — every check
  carries Critical/Warning/Info, an explanation, and a fix hint; new disk-space
  (2 GiB warn / 500 MiB critical) and model-file integrity checks (0-byte husks
  are critical); Fix All runs every available fix top-down, deduped.
- [x] **Daemon resilience batch** — tray heals a daemon that was down at launch
  (lazily-reconnecting bridge), transient whisper outages requeue with bounded
  attempts instead of failing recordings, retention honors delete_audio,
  wizard downloads are URL-allowlisted and only create files on success,
  open-file paths allowlisted, daily logs pruned to `log_max_files`.
- [x] **Diarization pipeline cached** — the ~500 MB speaker-diarization models
  used to reload on every diarized transcription; they now load once, lazily,
  into a config-keyed cache (speakrs' queued worker thread), serialize
  overlapping runs, invalidate on `[diarization]` changes, and never cache a
  failed load - a mid-session model download just works on the next run.
- [x] **Webhook SSRF guard + hook-test redaction** — webhooks classify their
  target before any bytes leave: loopback always allowed (local n8n/Home
  Assistant stay zero-config), private ranges need `[webhook]
  allow_private_network`, public hosts need https unless `allow_http`;
  hostnames resolve and the most restrictive class wins; redirects are no
  longer followed. Hook-test output is scrubbed of credential shapes
  (sk-/ghp_/AKIA/Bearer/key= and friends) before it reaches the UI.
- [x] **Pinned download checksums (S-H7)** — every wizard artifact (whisper GGML
  weights, the semantic model + tokenizer, the whisper-server zip) is verified
  against a pinned SHA-256 before use; the zip is checked before extraction,
  mismatches are deleted with a retry/compromised-mirror message, and an
  allowed-host URL without a pin fails closed (`src-tauri/src/checksums.rs`).
- [x] **Full-pipeline integration test** — transcribe → LLM stages → hook
  subprocess → webhook listener → catalog/inbox/audio, all asserted against
  fakes; plus tests for the wizard URL allowlist, `path_within`, and the
  notification contract.

### UX wiring

- [x] **Queue failed-items count + clear** — the queue panel surfaces the `failed/`
  count and lets you dismiss it (`QueuePanel.ts`).
- [x] **Import audio** button in Settings → Storage (`SectionStorage.ts`).

### Dictation (transcribe in place)

- [x] **Dictation fast lane** — in-place dictation skips the inbox queue entirely:
  own optional STT pick, instant rule-based polish (or LLM, or none), then types
  or pastes at the cursor before any library bookkeeping (`in_place.rs`,
  `[in_place]` config). Wispr-Flow-class latency, fully configurable.
- [x] **Type-first for the full pipeline** — with `[in_place] full_pipeline` on,
  `type_first` chooses when text lands: instantly from a type-only fast pass
  (cleanup/summary/tags catch up in the library) or only after the pipeline
  finishes (`pipeline_should_type`).

### Library & organization

- [x] **Auto-generated titles** — every recording gets a title: free first-clause
  heuristic by default (filler/annotations stripped, 60-char word-boundary cap),
  optional LLM titles; user-set titles always win (`title_is_auto` SQL guard);
  click-to-edit in the detail header (`phoneme-core::title`, `[title]` config).
- [x] **SRT / WebVTT caption export** — `phoneme export --captions <id>
  [--format srt|vtt] [-o FILE|-]` renders the stored segment timestamps as
  subtitles, speaker names prefixed (`phoneme-core::export`).
- [x] **Delete modes in the GUI** — delete everything, or keep the audio file and
  remove the recording from the library (the CLI's `--keep-audio`); one funnel
  for single/bulk/keyboard deletes, "don't ask again" remembers the chosen mode.
- [x] **Tag counts in the sidebar** — per-tag recording counts as quiet pill
  badges, case-insensitive tag identity, and a Settings action to clear ALL
  suggested tags across the library (`ClearAllTagSuggestions`).
- [x] **FLAC import** — wav / mp3 / m4a / flac, end to end (decoder feature,
  CLI + GUI filters, docs).

### Status, notifications & pickers

- [x] **Granular pipeline statuses** — recordings show cleaning up / summarizing /
  tagging (not just "processing"), driven by `PipelineStageChanged` events.
- [x] **Toast overhaul + step notifications** — errors time out (10 s), hover
  pauses with a countdown bar, stack capped; opt-in per-step completion toasts
  (`interface.step_notifications`), errors always surface.
- [x] **One provider/model picker everywhere** — the preset-vs-provider duality is
  gone: a single named-provider connection block (On this computer / Cloud /
  Custom, key row only when needed, "Get a key ↗", Test button, URL under
  Advanced) plus a shared model field with curated ⭐ suggestions per provider,
  identical across cleanup / summary / auto-tag / titles / STT / preview /
  re-run (`connectionField.ts`, `modelField.ts`).

### Recording

- [x] **Stop mode on the Record button** — the header dropdown picks how a voice
  note ends: on click, on silence, or after N seconds (the hotkeys' RecordMode,
  now clickable; persisted locally).

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
- [x] **Import audio file** — bring a `.wav`/`.mp3`/`.m4a`/`.flac` into the catalog (or `phoneme import <file>`) to queue it through the same transcription + hook pipeline as a live recording

---

## ✅ v1.7.1 — Local Intelligence & Internal Quality (shipped)

*Focus: solidify the full Windows feature set — especially local, on-device AI —
and pay down internal debt, so the v2.0 cross-platform port inherits a complete,
clean base.*

### Local AI (on-device, offline)

- [x] **Local semantic search** — bundle a local embedding model (e.g. all-MiniLM-L6-v2 via ONNX) + a vector index so you can search by *meaning* ("that idea about rust error handling last week"), not just exact text. Complements the existing FTS5 keyword search.
- [x] **Merged conversation view** — render a dual-track meeting as one exportable "You:" / "Meeting:" document, feedable to the LLM post-processor as a single context for summaries/action items. **Built on Lit (below), not raw `innerHTML`.** *(Note: as shipped this is a **coarse, source-sectioned, speaker-aware** merge — true line-by-line **chronological** interleaving by timestamp is still pending, because per-line timestamps aren't persisted. See the v1.9 Meetings roadmap item and [docs/design/merged-meeting-view.md](docs/design/merged-meeting-view.md).)*

### Internal quality

- [x] **Frontend reactivity (Lit for complex views)** — the framework-less `Store.ts` pattern is great for flat lists/forms and stays. But adopt **Lit (Web Components)** for the complex, dynamically-reconciled views (the merged conversation timeline first) to get declarative rendering + automatic lifecycle/listener cleanup without a full React/Vue. Do this *before* the merged conversation view.
- [x] **Test audio backend for full CI E2E** — the `Source` trait already abstracts capture (`CpalSource` prod, `SyntheticSource` tests), and Meeting Mode is end-to-end testable via `start_meeting_with_sources`. Extend the same injection to the **single-recording** daemon path so a CI test can drive CLI → daemon → (mock sine/silence) capture → SQLite without hardware, closing the "cpal device tests skipped in CI" gap.
- [x] **Typed errors** — `thiserror` for the library crates, `anyhow` in the binaries, for clean `?` propagation and better traces.
- [x] **Paginated recordings list** — `ListFilter` has `limit` but no `offset`, and the GUI fetches the list unpaginated. At 5,000+ recordings that floods the named pipe and hydrates thousands of `RecordingsList` rows at once, locking the UI thread and ballooning memory. Add `offset` to `ListRecordings` + catalog `list()`, plus a "Load More" / `IntersectionObserver` infinite scroll in `RecordingsList.ts`. (Pairs with the Lit adoption above.)

---

## ✅ v1.7.5 — Advanced Streaming & Diarization (shipped)

*Focus: Completion of the v1.7.x milestone — CI quality, UX polish, and internal hardening.*

- [x] **Synthetic audio CI backend** — full end-to-end CI test coverage via a `GeneratorSource` mock; drives CLI → daemon → capture → SQLite without hardware; closes the "cpal device tests skipped in CI" gap from v1.7.1.
- [x] **Meeting session indentation in recordings list** — expanded meeting groups visually indent their child tracks so standalone recordings are never confused with session members.
- [x] **rustfmt / CI hygiene** — formatter enforced on all modified files; all branches merged to master; `v1.7.5` tagged clean.
- [x] **Lit web component migration** — removed all Shadow DOM styling isolation issues across all Modals and Views.

*(Note: Local speaker diarization and real-time word-by-word transcription have been moved to the v2.0 backlog).*

---

---

*Planned work, v2.0, Long Term, Sustainability, and "Explicitly Not Doing" now live in [`ROADMAP.md`](ROADMAP.md).*
