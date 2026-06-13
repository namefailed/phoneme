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
- [x] **"More like this"** — open a recording → find semantically similar ones,
  for free: the recording's already-stored chunk vectors are averaged into the
  query (no fresh embedding; `catalog::more_like_this`, `MoreLikeThis` IPC) and
  the library is ranked by best-matching chunk, excluding the source and its own
  meeting's other track. **✨ Similar** button in the detail action row and the
  merged meeting view fills the list with the results (same relevance chips; a
  `~similar:` pill in the search box returns to the normal list); CLI parity via
  `phoneme search --like <RECORDING_ID>`. A not-yet-indexed recording reports it
  clearly instead of returning nothing.

### Meetings

- [x] **Merged meeting view** — selecting a meeting's group header opens a single,
  read-only reading of every track, labelled 🎤 Microphone / 🔊 System audio with the
  diarizer's `[Speaker N]` turns surfaced, plus Copy / Export
  (`MergedConversationDetail.ts`, `mergeMeeting.ts`). Coarse/source-sectioned — not
  yet chronologically interleaved.
- [x] **Speaker-diarization provider picker** — Settings → Transcription now exposes a
  Speaker Diarization section to choose who-spoke-when: off, **Local** (speakrs ONNX),
  **Deepgram**, or **AssemblyAI** (`SectionDiarization.ts`). Cloud diarization rides the
  same provider's transcription API, so the section shows a live warning when the chosen
  diarization provider can't run with the configured transcription backend (e.g. Deepgram
  diarization picked while Local transcribes) instead of silently doing nothing.
- [x] **Track-aware Meeting Mode** — a meeting's **mic track** is now labelled as one
  speaker, **You**, without running the diarizer at all; only the system/loopback track
  is diarized. The mic track is a single voice (yours), so diarizing it only burned time
  and produced spurious multi-speaker labels on one person. This halves a meeting's
  diarizer work and gives the mic track a clean, single-speaker transcript. The label
  reuses the canonical `[Speaker N]` machinery (a `speaker_names` row names label 1 → You),
  so it stays user-renamable and the merged-meeting view is unchanged. Normal single
  recordings and the system track are completely unaffected.

### Recording

- [x] **System-wide live-preview overlay** — an opt-in, always-on-top, frameless
  desktop window that floats the live caption over any app, even when the main
  window is hidden (`src-tauri/src/overlay.rs`, `frontend/overlay.*`); gated on
  `interface.preview_overlay`. Off by default.
- [x] **Word-level timestamps** — transcription providers now capture per-word
  timing (and per-word confidence from Deepgram/AssemblyAI) into a new
  `transcript_words` table, exposed via the `get_words` IPC request / Tauri
  command. The detail pane gains a **🔤 Synced** transcript peek: the machine
  transcript rendered as clickable, time-coded words — click a word to seek the
  waveform, and the word under the playhead highlights as audio plays. The shared
  substrate also unlocks confidence highlighting and tighter diarization
  boundaries.
- [x] **Word-level speaker attribution** — local diarization now assigns speakers
  **per word** instead of per whole segment. Building on the word-timestamp
  substrate, each word's time span is mapped onto the diarizer's per-frame
  activation matrix and attributed to the speaker who actually owns most of its
  frames, so a word straddling a hand-off lands on the right speaker (the case
  whole-segment labelling got wrong). Consecutive same-speaker words group into
  `[Speaker N]` turns for the transcript, the stored timeline, and the per-word
  speaker tags — all kept in agreement, and tied to the same stable numbering the
  segment path uses. Cloud and segments-only transcripts are unchanged: they fall
  back to the existing segment-level attribution, and a single-voice recording
  still reads as plain prose.
- [x] **Audio normalization** — optionally boost a quiet recording's gain to a
  consistent peak level before transcribing, so a soft microphone still hands
  Whisper a healthy signal (Settings → Capture → Recording; off by default,
  `recording.normalize` / `recording.normalize_target_dbfs`). Boost-only —
  silent or already-loud recordings are left untouched — and applied to the
  finalized capture (single recordings and each meeting track), never the live
  preview or imported files.

### Integration

- [x] **MCP server (`phoneme-mcp`)** — a thin Model Context Protocol bridge
  (JSON-RPC 2.0 over stdio) exposing five tools — `start_recording`,
  `stop_recording`, `get_transcript`, `search_recordings`, `list_recent` — that
  map straight onto the daemon's existing IPC requests with near-zero business
  logic. Observe-only (never spawns a daemon); a down/erroring daemon and any
  bad input surface as clean MCP tool errors, and the stdio framing is bounded
  (8 MiB, mirroring the IPC codec) against oversized or unterminated frames.
  Drop-in for Claude Desktop and other MCP hosts — see
  [MCP Server](docs/developer-guide/mcp_server.md).
- [x] **Local REST API (`phoneme-rest`, off by default)** — a localhost `axum`
  bridge over the daemon, bound to `127.0.0.1` only (the loopback trust
  boundary). Each endpoint maps one HTTP call to one `phoneme-ipc` `Request`
  (recordings list/get/segments, search, record start/stop, status, health), and
  `GET /api/events` streams `DaemonEvent`s as Server-Sent Events. Never
  auto-spawns the daemon (down → 503, bad input → 400), and the daemon
  round-trip is bounded so a wedged daemon can't hang a request. Opt-in via
  `[rest_api] enabled = true` (`port` default 3737). See
  [REST API](docs/developer-guide/rest_api.md).
- [x] **Webhook HMAC signing + custom headers** — when `[webhook] hmac_secret`
  is set, every outbound webhook POST carries an `X-Phoneme-Signature:
  sha256=<hex>` header (HMAC-SHA256 over the exact body bytes), so a receiver can
  verify the request genuinely came from this Phoneme install. `[webhook]
  custom_headers` attaches arbitrary `name = "value"` headers (e.g. an
  `Authorization` bearer), with collisions against headers Phoneme controls
  (`Content-Type`, the signature) ignored so they can't break the content type
  or forge the signature. The secret is DPAPI-encrypted at rest and masked at the
  WebView boundary like API keys; signing is off by default.

### Developer experience

- [x] `phoneme completions <bash|zsh|fish|powershell|elvish>` prints a shell-completion script to stdout (pure local, no daemon needed).

- [x] A fully-commented `config.example.toml` and `.env.example` at the repo
  root document every config key (with defaults) and every runtime env var.

### Performance

- [x] Semantic search holds the deserialized embedding corpus in memory, so
  repeated queries (and the upcoming RAG retrieval) skip re-reading and
  re-decoding every vector BLOB from SQLite; invalidated on any re-embed or
  delete, bounded for large libraries.

### Reliability & polish

- [x] Doctor's local whisper-server probes follow the port the server bound
  after a fallback (and say "running on 51234, fallback from 5809") instead
  of probing the dead configured port — fixed on both the daemon-side and
  tray-side Doctor paths.
- [x] Settings/wizard hints name the **effective** whisper port after a
  fallback ("running on 51234 — preferred 5809 was busy") instead of the
  configured one; the configured port stays editable.
- [x] Failures record their reason on the recording (survives a restart);
  cloud/custom transcriptions store the real model id instead of "unknown";
  failure toasts drop the internal "internal error:" prefix; the transcript
  diff computes once per refresh instead of twice.
- [x] The tray's daemon reconnect now backs off (250ms doubling to a 10s cap)
  instead of re-spawning and re-dialing on every action — a burst of UI
  clicks during a daemon outage no longer spawn-storms. A successful connect
  resets the backoff, and a daemon started later still heals on its own once
  the window elapses (the cap holds; it never permanently gives up).

### GUI parity

- [x] **Caption export in the GUI** — a 💬 Captions button on a transcribed
  recording's action row saves SubRip (.srt) or WebVTT (.vtt), matching
  `phoneme export --captions`.
- [x] **Webhook safety toggles** — Settings now exposes
  `webhook.allow_private_network` and `webhook.allow_http` (previously
  TOML-only) with plain-language warnings.
- [x] **Whole-library backup zip** — Settings → Storage → "Back up to .zip…"
  writes the same portable catalog+audio archive as `phoneme export <file>`
  (the old text-only Export is relabeled).

### CLI parity

- [x] **CLI reaches the GUI's per-recording actions** — `phoneme edit <id>
  --title/--clear-title/--favorite/--unfavorite`, `phoneme speaker
  rename|clear <id> <label> [name]`, `phoneme tag suggestions <id>
  [--approve|--dismiss <name>]`, `phoneme record --pause/--resume`, and
  `phoneme suggest-tags <id>` — all sending IPC requests that already
  existed, now reachable from the terminal.

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
- [x] **Doctor: provider-aware + triage layout** — checks now follow your
  actual providers: cloud STT swaps the local model/server checks for
  "API key configured" + "endpoint reachable" (a 401 still proves the wire;
  explanations say that's the most Doctor can verify without billing a
  request), per-step LLM connections are resolved (inheritance included),
  deduped per endpoint, and probed via free model-list routes. The
  diarization check now probes the Hugging Face cache the loader actually
  reads instead of the unwired local_model_path key. Both Doctor surfaces
  got a triage layout: sticky health strip with category count chips,
  failures first in full detail, passing checks folded into a grouped
  "<check> N passing" disclosure; re-runs no longer blank the list.
- [x] **Webhook SSRF guard + hook-test redaction** — webhooks classify their
  target before any bytes leave: loopback always allowed (local n8n/Home
  Assistant stay zero-config), private ranges need `[webhook]
  allow_private_network`, public hosts need https unless `allow_http`;
  hostnames resolve and the most restrictive class wins; redirects are no
  longer followed. Hook-test output is scrubbed of credential shapes
  (sk-/ghp_/AKIA/Bearer/key= and friends) before it reaches the UI.
- [x] **Bundled whisper-server ports fall back automatically** — 5809/5810 are
  now *preferred* ports: when another app already holds one, the daemon starts
  whisper-server on a free OS-assigned port instead (logged at warn), publishes
  the live value (`daemon_status` reports preferred + effective per server),
  and every consumer — final transcription, live preview, dictation, the
  Settings/wizard "Test" probe — dials the effective port. The preview's
  choice can never collide with the main server's
  (`whisper_supervisor.rs`, `app_state.rs`).
- [x] **Audit wave C hardening** — five reliability fixes from the code audit:
  - *WAV atomic write* — recordings are written to a `.tmp` sibling and renamed
    into place; a crash mid-write never leaves a corrupt WAV at the final path
    (also handles Windows rename semantics correctly).
  - *IPC accept backoff* — repeated `accept()` failures on the named-pipe
    listener now back off exponentially (up to 4 s) instead of looping
    immediately, preventing a busy-spin during transient handle exhaustion.
  - *Config reload by mtime* — the queue worker compares the config file's
    modification time before parsing it; unchanged files skip the TOML parse
    entirely. The diarizer-cache invalidation hook still fires on real disk
    changes.
  - *No-spawn read-only commands* — `list`, `show`, `search`, `doctor`, `queue
    list/counts/status`, `daemon status`, and `watch` no longer silently start
    the daemon when it isn't running; they report "daemon not reachable" and
    exit non-zero, making the daemon's state visible instead of masking it.
  - *Idempotent crash recovery* — if the daemon crashes in the window between
    writing `done/<id>.json` and removing `processing/<id>.json`, startup
    recovery now detects the done+processing pair and drops the stale
    processing file instead of re-queuing the already-completed item.
- [x] **Audit wave — status semantics, filters, config tooling, perf** — six
  fixes from the code audit:
  - *A real `Cancelled` status* — cancelling a queued or in-flight recording
    now marks it `cancelled` (its own quiet gray pill, a status-filter entry,
    CLI rendering) instead of borrowing `transcribe_failed`; cancelled
    recordings never appear in the failed panel or count as failures, and
    retention treats them as terminal like done/failed. Wire/DB string is
    `"cancelled"`; the string status column needs no migration.
  - *Server-side kind/favorite filtering* — `ListFilter` gained `kind`
    (`single`/`meeting`) and `favorite` flags applied in SQL before
    LIMIT/OFFSET; the GUI Library filter and `phoneme list --kind` ride them,
    so Favorites/Meetings pages deep into a large library come back full
    instead of mostly empty (the old client-side post-pagination filter
    remains only as a fallback for older daemons).
  - *`config set` honors `PHONEME_CONFIG`, validates, writes atomically* — it
    now writes the same file the daemon resolves (env override first), runs
    the full `Config::validate()` parse-back before touching disk (a bad value
    can no longer brick the config), and replaces the file via tmp+rename.
  - *`doctor --rebuild-catalog` no longer races the daemon's shutdown* — it
    waits (bounded, 15s) for the daemon's pipe to actually vanish before
    deleting `catalog.db` (now including the `-wal`/`-shm` sidecars), and
    refuses to touch the files if the daemon won't exit.
  - *Embedding backfill no longer blocks config reloads* — the startup chunk
    -embedding backfill and the `ReembedAll` sweep re-acquire the embedder
    read lock per item instead of holding it across the whole loop, so a
    Settings save mid-backfill applies immediately instead of waiting minutes.
  - *`ipc_handler` deduplicated* — the repeated error/ok/not-found response
    shapes are factored into three helpers (`err_response`, `not_found`,
    `ok_null`), byte-identical on the wire, dropping ~8 KB of boilerplate.
- [x] **Audit wave — capture + daemon correctness** — six fixes from the code
  audit:
  - *Loopback gap filling runs on the audio clock* — the silence inserted to
    keep a meeting's system track continuous is now sized against what the
    device actually delivered (counted at the capture callback) instead of
    wall-clock elapsed vs. processed samples; CPU load that delays the audio
    worker can no longer read as a fake gap and stuff extra silence into the
    track (which ran it long and desynced the meeting).
  - *Every device sample format captures* — recording used to support only
    f32/i16 devices; all formats cpal can report (i8/i16/i32/i64, u8/u16/u32/
    u64, f32/f64) now convert through one lossless path, and a truly unknown
    format fails with an error naming the format and the device instead of a
    generic refusal.
  - *Meeting stop finalizes tracks independently* — one track failing to
    write/enqueue no longer abandons the other mid-stop: each track is
    finalized on its own, failures land on the normal `transcribe_failed`
    path, and only a meeting where *every* track failed reports an error.
  - *Status IPC no longer stalls behind stop* — `record stop`/`cancel` release
    the active-recording lock before tearing down the live-preview loop, so a
    preview stuck in a slow transcription tick can't freeze every
    status/pause/cancel request for its duration.
  - *Doctor restarts cancel the whisper backoff* — a restart request that
    arrived while a supervisor was sleeping out its crash backoff (up to 60 s)
    was silently lost; the backoff now listens and respawns immediately (both
    the main and preview server loops, and shutdown cancels the wait too).
  - *Version-mismatch restart spares in-flight work* — the tray no longer
    bounces an older-version daemon that is mid-recording or mid-transcription
    (the restart killed the capture); it proceeds against the old daemon and
    the upgrade happens at the next idle start. The CLI's blocking `record`
    also subscribes to events *before* sending stop, so a fast transcription
    finishing in that gap can't leave it hanging to timeout.
- [x] **Pinned download checksums (S-H7)** — every wizard artifact (whisper GGML
  weights, the semantic model + tokenizer, the whisper-server zip) is verified
  against a pinned SHA-256 before use; the zip is checked before extraction,
  mismatches are deleted with a retry/compromised-mirror message, and an
  allowed-host URL without a pin fails closed (`src-tauri/src/checksums.rs`).
- [x] **Full-pipeline integration test** — transcribe → LLM stages → hook
  subprocess → webhook listener → catalog/inbox/audio, all asserted against
  fakes; plus tests for the wizard URL allowlist, `path_within`, and the
  notification contract.

### Lifecycle — full shutdown chain + Ollama auto-launch

- [x] **Quit stops everything Phoneme started** — tray Quit (default
  `interface.quit_stops_daemon = true`) sends the daemon a graceful Shutdown
  and waits for it to vanish: an in-flight recording is stopped and queued
  through the normal recorder path first (transcribed on the next start),
  then the whisper-server(s) and a Phoneme-launched Ollama go down with the
  daemon. Set the knob to `false` for the old headless behavior — the daemon
  outlives the tray (`tray.rs`, `lib.rs`).
- [x] **End-process robustness via Job Objects** — the daemon holds a
  kill-on-close job every child it spawns joins (whisper main + preview, an
  Owned Ollama), and the tray (when `quit_stops_daemon` is on, decided at
  spawn time) holds one for the daemon — so even Task Manager's End task
  reaps the whole tree (`phoneme-core::job`, `whisper_supervisor.rs`,
  `auto_spawn.rs`).
- [x] **Ollama auto-launch with an ownership ledger** — when an LLM step
  (cleanup, summary, tags, titles, in-place polish) resolves to a **local**
  Ollama that isn't running, the daemon launches `ollama serve` on demand
  (`[llm_post_process] autostart_ollama`, default on), waits for readiness,
  and logs the server to `logs/ollama.log`. The ledger makes ownership
  sticky: an Ollama that was already running at first probe is NotOurs
  forever — never killed, never restarted, never job-assigned; only a
  daemon-launched one is Owned and reaped at shutdown. Single-flight, so
  concurrent steps can't double-spawn (`ollama_launcher.rs`).
- [x] **Shutdown acknowledges before exiting** — the `shutdown` IPC writes its
  Ok response, then tears down after a short grace, so `phoneme daemon stop`
  and the tray never hang on a dead pipe; `daemon stop` now waits for the
  pipe to actually vanish and reports `daemon stopped` (and stopping a
  stopped daemon is a clean no-op instead of auto-spawning one).

### UX wiring

- [x] **Queue failed-items badge + failure details** — the queue panel surfaces the
  `failed/` count; clicking the badge opens a details panel: one row per failed
  recording with the step that broke (Transcription / Hook), the error text
  (captured live off the failure events; selectable), and when — per-row **Retry**
  (re-runs the whole pipeline) and **Open**, a sequential **Retry all** with a
  progress count, and the quarantine **Clear failed** moved into the footer (the
  recordings keep their Failed status and stay in the library)
  (`QueuePanel.ts`, `FailedPanel.ts`).
- [x] **`phoneme queue skip`** — CLI parity for the queue panel's ⏭: skips the
  LLM step (cleanup / summary / tagging) currently running for the active item
  (`SkipCurrentStage` IPC). Observe-only — it never auto-spawns a daemon just
  to skip nothing.
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
- [x] **Saved-search rename collision guard** — renaming a saved search to a
  name another one already uses is refused with a clear toast (the rename editor
  stays open) instead of silently leaving two same-named searches where the next
  save overwrites whichever sits first (`savedSearches.ts`).
- [x] **Compare versions survives hour-long transcripts** — the version diff
  peels off the common prefix/suffix and caps the LCS table (`MAX_LCS_CELLS`);
  an oversize word diff degrades to line then block granularity with an in-view
  notice instead of freezing the webview (`utils/diff.ts`, `TranscriptDiff.ts`).
- [x] **Sturdier tag-suggestion parsing** — the tagger finds the first *valid*
  JSON array in a chatty model reply; bracket-bearing prose around it ("[1] as
  cited…", "[hope that helps]") no longer derails parsing into junk tag
  candidates (`pipeline.rs parse_tag_names`).

### Status, notifications & pickers

- [x] **Granular pipeline statuses** — recordings show cleaning up / summarizing /
  tagging (not just "processing"), driven by `PipelineStageChanged` events.
- [x] **Toast overhaul + step notifications** — errors time out (10 s), hover
  pauses with a countdown bar, stack capped; opt-in per-step completion toasts
  (`interface.step_notifications`), errors always surface.
- [x] **Skipping a step no longer reads as a failure** — the queue panel's ⏭
  (and `phoneme queue skip`) used to end in an error toast ("Summary failed:
  …step skipped by user"); a user skip now toasts "Summary skipped" (info,
  step-gated) while real summary failures keep erroring (skip sentinel in
  `pipeline.rs`, toast routing centralized in `notifications.ts`).
- [x] **Health pill polls only while visible** — the header's 30-second Doctor
  poll probes backends, so it now pauses while the window is hidden/minimized
  and re-checks the moment the window shows again (`HeaderBar.ts`).
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
