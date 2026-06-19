# 🗺️ Phoneme Roadmap

Where Phoneme is **going**. This page is forward-looking only.

- **Shipped history** lives in [`CHANGELOG.md`](CHANGELOG.md) — the canonical record of what's done.
- **Speculative / unvetted** ideas wait in the [Idea Parking Lot](docs/IDEAS.md) until they earn a place here.
- **Ideas we've pushed back on** (with reasons — but nothing's permanent) are in [Not convinced yet](#-not-convinced-yet) at the bottom.

**Confidence tags** — every item carries one:

- 🔨 **Committed** — in flight or next up; we intend to ship this soon.
- 📋 **Planned** — on the roadmap and intended, not yet started.
- 🔬 **Exploratory** — a bet we'd love to make; needs a prototype or a real user ask before it's promoted.

A ⚠️ marks an item that **depends on shared substrate** built by another item (build the substrate once).

> **This is a living document, not a contract.** Tags are a current read, not a
> promise, and even the *Not convinced yet* list isn't a wall — Favorites lived
> there once, a real case appeared, and we shipped it. If something genuinely
> makes sense, we do it; if a "planned" item stops earning its place, it goes.

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
   greenfield engineering. Close that gap first.
3. **Differentiation lives in Recall and Meetings**, not Dictation. Dictation is
   table-stakes (keep it frictionless); the moat is "search/ask your own voice
   archive" and "dual-track diarized meetings."
4. **Respect dependencies.** Some "separate" features share one substrate — build
   the substrate once (the ⚠️ callouts below mark where).

---

## 🔜 Horizon 0 — Now / Next

The committed work that's in flight or immediately next. This is the active work list.

### Diarization quality
*The prerequisite for trustworthy Meetings — don't build naming UX on wrong labels.*

- 🔨 **Expose `PipelineConfig` tunables** — surface speakrs `merge_gap` / `speaker_keep_threshold` in Settings via `run_with_config` (the pipeline uses `default_config` today). Cache invalidates on change. *(Not a Cpu/CpuFast toggle — that mode doesn't exist on Windows/CPU.)*
- 🔨 **Selectable local diarization models** — a `diarization.models_dir` override (`PipelineBuilder::from_dir`) with a Settings field + Doctor check; later, curated alternative bundles in the wizard (pinned SHA-256, after verifying ONNX shapes match speakrs).
- 🔨 **Automatic named-speaker recognition** ⚠️ *(builds on the embeddings substrate)* — aggregate per-cluster voiceprint centroids, persist per name, cosine-match on later recordings. Manual rename already ships; this makes it stick across recordings.
- 🔨 **DER eval harness** — an RTTM fixture set + a dev-only harness behind speakrs's `_metrics` feature, wired as an optional nightly job (not a release gate).

### Dictation
- 🔨 **Streaming-type spike** — type words as they finalize instead of all-at-end (corrections vs. cursor-churn trade-off). A reserved `in_place.stream_type` no-op flag already exists; this is the experiment that decides whether it ships. *(All other in-place phase-2 work — voice commands, per-app delivery, app-aware context, the waveform overlay — has shipped.)*

### Foundation & tech-debt
- 🔨 **Integration + E2E test coverage** — the biggest untested critical path: a synthetic-audio E2E for the single-recording path (incl. track-from-source), the full transcribe → LLM → hooks → webhook → catalog pipeline, and meeting capture.
- 🔨 **Webhook resilience** — retry/backoff + rate limiting / circuit breakers for external services (OpenAI, Ollama, webhooks).
- 🔨 **Replace remaining `unwrap()`** in production paths (recorder source opens, remaining hot paths); the model-override readiness race (audit A2-M7).
- 🔨 **Small decisions & wiring** — Quick-Switcher "Save as default" recipe-awareness · Doctor: skip/downgrade local-whisper checks when a cloud STT provider is configured · configurable webhook trigger point (before/after/independent) · per-item failed-quarantine dismiss · persist saved searches in the catalog (webview today).

---

## 🎯 Horizon 1 — Deepen the moat (Recall · Meetings · Intelligence)

Where Phoneme wins. Mostly net-new capability on top of the substrate that now exists.

### Recall librarian
- 📋 **Ask my archive** — a local RAG chat over your transcripts ("What did we decide about the API last week?"), grounded with citations back to the recordings. The flagship Recall feature.
- 🔬 **Chat with this transcript** — per-recording Q&A; a lighter, scoped version of Ask-my-archive for a single note or meeting.
- 🔬 **Entity extraction → faceted search** — pull people / projects / dates out of transcripts into filterable facets.
- 🔬 **Topic timelines** — "everything I said about X, in order" — a chronological thread across recordings.
- 🔬 **Auto-linking / "see also"** — a knowledge graph of related recordings surfaced in the detail pane (the chunk-embedding substrate makes this cheap).
- 🔬 **Smart collections** — auto-populating folders from saved-search rules or semantic clusters.
- 🔬 **Daily / weekly digest** — a generated rollup of what you recorded.
- 🔬 **Tasks / reminders from voice notes** — "remind me to…" → an action item or an export to your todo system.

### Meeting archivist
- 📋 **Whole-meeting digest** — one summary across both tracks / the merged You↔Meeting timeline.
- 📋 **Speaker enrollment / voice library** ⚠️ *(builds on Horizon 0 named-speaker recognition)* — name a voice once → recognized across **all** future meetings.
- 🔬 **Meeting templates** — standup → structured action items, interview → Q&A; recipe-driven, selectable per meeting hotkey.
- 🔬 **Live action-items & decisions** — extract them as the meeting happens (a real-time assistant), not only after.
- 🔬 **Calendar-based naming & auto-start** — name a meeting from its calendar event; optionally prompt to record at a scheduled call. *(Calendar-driven — not the rejected process-sniffing.)*
- 🔬 **Per-app audio capture** — capture only the call app's audio (not your music) via per-session loopback.
- 🔬 **Chapter markers** *(parked)* — split a long meeting on silences into navigable chapters. Promote when someone records genuinely long sessions.
- 🔬 **Duplicate detection** *(parked)* — "you already recorded this call" on import/start.

### Dictator
- 📋 **App-aware context, tier 2** — opt-in screenshot → vision-LLM context for the polish prompt (tier 1, window-title context, already ships). The trust-sensitive, later half.
- 🔬 **Per-app tone / register** — adapt the polish style (email vs. code vs. prose), not just jargon, per foreground app.
- 🔬 **Real-time translation dictation** — speak one language, type another (Whisper translate path).
- 🔬 **Snippet / macro expansion** — "insert my signature / address" expanded inline during dictation.
- 🔬 **User-defined dictation commands** — extensible phrase → edit beyond the built-in "new line / scratch that". *(Distinct from the rejected always-listening wake word.)*

### Intelligence / engine
- 📋 **Forced re-alignment of edited transcripts** ⚠️ *(needs a forced-aligner dependency)* — re-derive word timings after a hand edit so the Synced/Timeline views stay precise.
- 🔬 **Streaming summaries** — the summary builds as you record, not only at the end.
- 🔬 **Local LLM model manager** — download/manage Ollama models in-app, the way Whisper models already work.
- 🔬 **Confidence-driven re-transcription** — auto-flag low-confidence spans for review or a targeted re-do.

---

## 🌐 Horizon 2 — Meet users where they are (Platform · Ecosystem)

### Platform & ports
- 📋 **macOS port** — Apple-Silicon first, bundled whisper.cpp; ship microphone-only first (system-audio loopback needs a virtual device on macOS — don't let Meeting Mode block launch).
- 📋 **Linux port** — PipeWire / ALSA (PipeWire monitor sources give system-audio loopback natively); X11 + Wayland global hotkeys.
- 📋 **Windows ARM** — native ARM64 build for Snapdragon machines.
- 📋 **Multi-microphone** — capture two input devices at once (two-person in-person interviews).
- 📋 **Browser extension** ⚠️ *(rides the REST API, which ships)* — toolbar click → record → paste into the focused field.
- 🔬 **Mobile thin-client / companion** — records on the go, syncs to the desktop daemon over LAN; transcription runs on the desktop.
- 🔬 **Mobile-responsive web view** — make the existing web peer usable on a phone.

### Automator ecosystem
- 📋 **Per-recording hook override** — this one goes to Discord, that one stays local (per-recipe hooks already enable per-hotkey routing; this is the after-the-fact per-recording control).
- 🔬 **Recipe marketplace / sharing** — export/import recipes; a community library.
- 🔬 **Conditional / branching recipes** — "if transcript contains X → hook A else B" (extends keyword triggers to full conditionals).
- 🔬 **First-class integrations** — Obsidian / Notion / Logseq / Todoist beyond raw hooks.
- 🔬 **Launcher integrations** — Raycast / PowerToys Run / Alfred quick-record.
- 🔬 **Zapier / n8n nodes** — native automation-platform connectors.
- 🔬 **Plugin ecosystem** — a standardized API + JSON registry for community hooks / themes / post-processors.

### In-app intelligence
- 📋 **Phoneme Agent** — a fully phoneme-aware agent inside the app ("summarize my last five standups", "tag everything about the migration"), driving the same surface as the MCP/REST tools.

---

## 🔭 Horizon 3 — Scale, privacy & foundation

### Data & privacy
- 📋 **Cloud sync (opt-in, user-controlled)** — encrypted sync of the catalog to a user-owned S3/Backblaze bucket; audio excluded by default.
- 🔬 **Local encrypted vault** — encrypt audio + catalog at rest (opt-in), beyond the per-key DPAPI encryption that already ships.
- 🔬 **Hard local-only mode** — a network kill-switch guarantee for sensitive users (no provider call can leave the box).
- 🔬 **Retention policies per tag / per profile** — finer-grained than the global age/count auto-delete.
- 🔬 **Data-access transparency log** — what hook / AI step touched which recording.

### Teams & cloud
- 🔬 **Phoneme Cloud (optional, self-hostable)** — shared catalogs + role-based access for teams; the desktop daemon stays fully offline-capable.

### Foundation & DX
- 📋 **Playwright E2E UI coverage** — a full suite driving the frontend against the real Rust backend over IPC (after the architecture stabilizes across platforms).
- 🔬 **Command palette** — everything reachable by typing.
- 🔬 **Accessibility pass** — screen-reader support (NVDA/JAWS), ARIA labels, font scaling, high-contrast theme.
- 🔬 **Protocol versioning** — a version field on the IPC Request/Response contract.
- 🔬 **Batch operations** — batch delete / batch tag update over a selection.
- 🔬 **Scale to 100k+ catalogs** — DB vacuum strategy, indexing, quoted-FTS5 phrase search.
- 🔬 **Internal tidy-ups** — split the large modules (`config.rs`, `catalog.rs`, `recorder.rs`), `cargo-deny` + coverage in CI, redundant-clone perf trims.

### Sustainability & monetization
*Optional, opt-in, and never at the expense of the local-first core.*

- 🔬 **Phoneme Pro (managed APIs)** — zero-config cloud Whisper + premium LLM cleanup without managing keys.
- 🔬 **Phoneme Sync** — end-to-end-encrypted catalog + audio sync across machines.
- 🔬 **Phoneme for Teams** — per-seat managed backend; centralized, searchable, role-governed meeting notes.
- 🔬 **Paid mobile companion** — a thin-client that records on the go and syncs back to the desktop daemon.

---

## 🤔 Not convinced yet

We weighed these and weren't sold — the *why* is kept below so we don't re-litigate
them on a whim. But **nothing here is permanent.** Favorites sat in this list once;
a real case showed up and we shipped it. The bar is the same as everything else
(Guiding principle #1) — if a real user actually hits the friction, or a cheap path
appears, any of these can graduate to a horizon above. Read it as *current
skepticism with reasons*, not a ban. (Earlier-stage speculative ideas live in the
[Idea Parking Lot](docs/IDEAS.md).)

| Idea | Why we've pushed back (for now) |
|------|--------|
| Duration filter | Niche; nobody asked; search + tags already narrow the list |
| Backup/restore ZIP | Manual export covers it; the SQLite DB is a single copyable file |
| Azure Speech / AWS Transcribe | Enterprise pricing; not the target user; add only on demand |
| Portable (unsigned) ZIP | A CI task, not a product feature |
| Winget / Scoop packages | Same — packaging automation, not a roadmap item |
| Meeting-app awareness (auto-detect Zoom/Teams) | Brittle, false-positive-prone, and surveillance-y for a privacy-first app |
| Voice commands / wake word | Push-to-talk already solves hands-free; wake-word is a false-trigger rabbit hole |
| Transcript git-style version graph | YAGNI at this scale; original-vs-current diff covers ~95% |
| Acoustic echo cancellation (speaker→mic bleed) | Genuinely hard research problem; honest answer is "wear headphones" |
| Word-by-word streaming transcription | The bounded live preview covers the need; full real-time captioning is a v2.0-grade rabbit hole |
