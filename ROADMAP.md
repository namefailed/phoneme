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

- 🔨 **Named-speaker recognition — tuning & validation** — the recognition stack itself **shipped** (voiceprint capture → enroll-on-rename → cosine match → GUI suggestions; see CHANGELOG). What remains is *forward* work: empirically calibrate the merge + match cosine thresholds (both default `0.5`, hand-tuned on a small sample) on real multi-speaker recordings, a native test pass, and tagging stored voiceprints with the embedding-model version so a `models_dir` swap can't silently match against incompatible centroids.
- 🔨 **DER eval harness** — an RTTM fixture set + a dev-only harness behind speakrs's `_metrics` feature (or a reimplemented collar-0 DER), wired as an optional nightly job (not a release gate).
- 🔬 **Curated diarization model bundles** — selectable alternative speakrs bundles in the setup wizard (pinned SHA-256, after verifying ONNX shapes match). *(The `diarization.models_dir` override that loads a custom bundle already ships; this is the curated-catalog half.)*
- 🔨 **In-recording speaker correction — detail-pane editor** — the backend + wire contract **shipped** (reassign / merge / split a recording's speakers: `catalog::{reassign_segment,merge_speakers,split_speaker}`, `ReassignSegmentSpeaker` / `MergeSpeakers` / `SplitSpeaker` IPC + Tauri commands, `phoneme speaker reassign|merge|split`; segments stay authoritative and the prose `[Speaker N]:` markers rebuild in-transaction — see CHANGELOG). What remains is the **interactive merge/split UI** in the recording detail/merged view (click a turn → reassign; select turns → split; pick two speakers → merge), which needs a native design pass. It calls the requests above as-is.

### Foundation & tech-debt
- 🔨 **Replace remaining `unwrap()`** in production paths (recorder source opens, remaining hot paths); the model-override readiness race. *(~799 `unwrap()/expect()` in `phoneme-core` + `phoneme-daemon`; a panic in a critical task takes the whole daemon down. Add a `clippy::unwrap_used` lint so new ones can't creep in.)*
- 🔨 **Kill the hook double-fire hazard** — the legacy hook loops still execute in `pipeline::run` alongside the recipe-driven `run_hook_steps`; a partial/stale config (`hooks_migrated = true` + non-empty `hook.commands`) can fire shell hooks **twice** (arbitrary-shell, fires-outside-the-recipe-model). Fold them behind a post-migration empty-assert, or delete now that migration is idempotent.
- 🔨 **Make `phoneme-agent-core` load-bearing** ⚠️ *(substrate for the H2 in-app Phoneme Agent)* — the crate is the compile-checked tool seam the planned in-app agent will drive, but nothing consumes it yet and it has already drifted from its hand-duplicated `phoneme-mcp` twin (16 tools vs 18). Wire `phoneme-mcp` to consume its registry so there's one source of truth and a renamed `Request` breaks the build — that protects the shipping MCP surface now and leaves the agent a ready substrate. (Keep it; it's pre-built for the agent — just stop maintaining the twin by hand.)
- 🔨 **IPC protocol version field** — add a numeric `protocol_version` to the daemon handshake and refuse/warn on mismatch. Today compatibility rests only on additive serde + a version *string*; one non-additive schema change silently breaks all four surfaces (tray/CLI/MCP/REST) at once, and the updater can leave a newer tray talking to an older busy daemon. Cheap now, expensive per added surface. *(Pulled up from H3.)*
- 📋 **De-duplicate hand-synced trust logic** — Doctor's per-step LLM resolution (`doctor.rs step_llm_connection`) and the pipeline's `*_llm_config` independently re-derive provider inheritance (drift → Doctor probes a different connection than the pipeline uses); likewise the canonical `needed_whisper_servers` vs the supervisor's hand-rolled per-loop gates. Make each one source of truth.
- 📋 **CI hardening** — flip `cargo audit` / `pnpm audit` from advisory-only to blocking for the core crates (already planned in a `ci.yml` comment), and fix test isolation (per-test temp DB via the `PHONEME_DATA_LOCAL` override) so `--test-threads=1` can go away and parallel CI catches isolation rot instead of masking it (the SQLite race).
- 🔬 **Opt-in, local-only diagnostics bundle** — a Doctor button that exports a sanitized log-tail + config for bug reports. "No telemetry" (correct) ≠ "no diagnostics": today a field panic is invisible to you and looks like "it just died" to the user. No network; fully consistent with the privacy posture.
- 🔬 **Quick-Switcher recipe-awareness** — let the header "Save as default" quick switcher surface (and switch) the active recipe, not just the model. *(Needs a small design pass — normal recordings always run the fixed `default` recipe today, so it isn't yet clear what this would set.)*

---

## 🎯 Horizon 1 — Deepen the moat (Recall · Meetings · Intelligence)

Where Phoneme wins. Mostly net-new capability on top of the substrate that now exists.

### Recall librarian
- 📋 **Ask my archive** — a local RAG chat over your transcripts ("What did we decide about the API last week?"), grounded with citations back to the recordings. The flagship Recall feature. It's retrieval-and-answer, not an agent: it rides the existing hybrid search + embeddings (retrieval) and `[llm]` providers (generation) behind one new request — **no tool-calling, so it needs neither the MCP server nor `phoneme-agent-core`**. The net-new work is the RAG prompt assembly, citation mapping, and chat UI (+ the ANN index below for retrieval that scales).
- 🔬 **Chat with this transcript** — per-recording Q&A; a lighter, scoped version of Ask-my-archive for a single note or meeting.
- 🔬 **Entity extraction → faceted search** — pull people / projects / dates out of transcripts into filterable facets.
- 🔬 **Topic timelines** — "everything I said about X, in order" — a chronological thread across recordings.
- 🔬 **Auto-linking / "see also"** — a knowledge graph of related recordings surfaced in the detail pane (the chunk-embedding substrate makes this cheap).
- 🔬 **Smart collections** — auto-populating folders from saved-search rules or semantic clusters.
- 🔬 **Daily / weekly digest** — a generated rollup of what you recorded.
- 🔬 **Tasks / reminders from voice notes** — "remind me to…" → an action item or an export to your todo system.
- 📋 **Vector (ANN) index** ⚠️ *(prerequisite for "Ask my archive" at scale)* — replace the brute-force O(N) cosine scan with `sqlite-vec` / HNSW. Today every semantic query re-scans the whole corpus, and past the 200k-vector cache cap it silently degrades to re-decoding every f32 BLOB from SQLite *per query* — so the moat feature slows down exactly as the archive that makes it valuable grows. Build this *before* Ask-my-archive ships onto it. *(Pulled up from H3 "scale to 100k".)*
- 📋 **Meaning-search + filters together** — let semantic search honour the same tag / date / status / favorite filters `list()` already supports; today they're mutually exclusive, so you can't ask "meaning + last week, tagged work". Backend-mostly — the filter chips already exist in the UI.
- 🔬 **Find-and-replace across a transcript (and library-wide)** — correct a recurring mis-transcription once, optionally across all recordings. Custom vocabulary only biases *future* decodes; this fixes the existing archive. Pairs with the FTS index.
- 🔬 **Pinned recordings** — a true pin that floats a few reference recordings to the top of any view (favorites is a *filter*, not a pin).

### Meeting archivist
- 📋 **Whole-meeting digest** — one summary across both tracks / the merged You↔Meeting timeline.
- 📋 **Speaker enrollment / voice library** ⚠️ *(builds on Horizon 0 named-speaker recognition)* — name a voice once → recognized across **all** future meetings.
- 🔬 **Meeting templates** — standup → structured action items, interview → Q&A; recipe-driven, selectable per meeting hotkey.
- 🔬 **Live action-items & decisions** — extract them as the meeting happens (a real-time assistant), not only after.
- 🔬 **Calendar-based naming & auto-start** — name a meeting from its calendar event; optionally prompt to record at a scheduled call. *(Calendar-driven — not the rejected process-sniffing.)*
- 🔬 **Per-app audio capture** — capture only the call app's audio (not your music) via per-session loopback.
- 🔬 **Chapter markers** *(parked)* — split a long meeting on silences into navigable chapters. Promote when someone records genuinely long sessions.
- 🔬 **Duplicate detection** *(parked — embedding substrate now met, awaiting a real "I have dupes" complaint)* — "you already recorded this call" on import/start.
- 🔬 **Audio clip export ("share this moment")** — select a transcript span → export just that audio segment + its caption as a small file. Word timings already map text↔audio, so the cut points are free; stays local-first (exports a file, no cloud). Fathom/Grain built a business on this.

### Dictator
- 📋 **App-aware context, tier 2** — opt-in screenshot → vision-LLM context for the polish prompt (tier 1, window-title context, already ships). The trust-sensitive, later half.
- 🔬 **Per-app tone / register** — adapt the polish style (email vs. code vs. prose), not just jargon, per foreground app.
- 🔬 **Real-time translation dictation** — speak one language, type another (Whisper translate path).
- 🔬 **Snippet / macro expansion** — "insert my signature / address" expanded inline during dictation.
- 🔬 **User-defined dictation commands** — extensible phrase → edit beyond the built-in "new line / scratch that". *(Distinct from the rejected always-listening wake word.)*
- 🔬 **Dictation history / re-grab last** — a quick popover of recent typed snippets to re-paste when focus was lost or the wrong app got it (the #1 dictation failure mode). Wispr Flow keeps a history for exactly this; dictations are already saved, there's just no fast re-grab affordance.
- 🔬 **Filler-word removal as a deterministic Playbook Transform** — a pure-Rust "strip um/uh/like" move, separate from LLM cleanup. Fast, free, offline, deterministic; category-standard (Otter, Descript). Today it only happens implicitly inside the slow LLM pass.
- 🔬 **Spoken-language detection → routing** — act on Whisper's auto-detected language per dictation (surface/route) instead of the single configured BCP-47 hint. Detection only; distinct from the translation-dictation item above.

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
- 🔬 **Per-hook-step run log** — persist each Hook step's exit code, stdout tail, and timing per recording (today multiple steps collapse to one provenance row). The core of the Automator's "debuggable" promise.
- 🔬 **Shell-hook retry/backoff** — local script hooks fail on a single attempt while webhooks already retry; reuse the webhook backoff so a momentarily-locked Obsidian vault doesn't silently drop the action.
- 🔬 **Recipe dry-run / preview** — run a recipe end-to-end against a sample transcript before binding it (only single-hook test exists). Authoring blind and discovering it's wrong on the next real recording is poor UX for the persona whose whole job is configuration.

### In-app intelligence
- 📋 **Phoneme Agent** ⚠️ *(builds on the H0 `phoneme-agent-core` consolidation)* — a fully phoneme-aware agent inside the app ("summarize my last five standups", "tag everything about the migration") that *acts*, not just answers. Unlike Ask-my-archive this needs tool-calling, so it's built **on `phoneme-agent-core`** — the in-process tool seam (tool schema → typed daemon `Request`) — **not** on `phoneme-mcp`, which is the *external* door that lets outside hosts (Claude Desktop) drive Phoneme. Same tool registry, opposite directions: once MCP consumes agent-core there's one catalog feeding both. Net-new here is the agent loop (LLM ↔ tool-call orchestration), conversation state, and chat UI; retrieval becomes one of the agent's tools, so RAG and actions combine.

---

## 🔭 Horizon 3 — Scale, privacy & foundation

### Data & privacy
- 📋 **Cloud sync (opt-in, user-controlled)** — encrypted sync of the catalog to a user-owned S3/Backblaze bucket; audio excluded by default.
- 🔬 **Local encrypted vault** — encrypt audio + catalog at rest (opt-in), beyond the per-key DPAPI encryption that already ships.
- 🔬 **Hard local-only mode** — a network kill-switch guarantee for sensitive users (no provider call can leave the box).
- 📋 **Pre-send cloud-egress guard** — a *backend* gate (per-recording confirm, or a tag-based block) before audio leaves the box to a cloud STT/LLM. Today the egress warning is UI-only — nothing stops a sensitive/meeting recording reaching a configured cloud provider. Narrower and cheaper than the hard local-only mode, and closes a real hole in a privacy-first product.
- 🔬 **Per-recording local/cloud provenance badge** — 🔒 fully-local vs ☁️ touched-a-cloud-provider, derived from the model provenance already recorded. A lightweight, honest precursor to the transparency log below.
- 🔬 **Soft-delete / trash with a recovery window** — deleted recordings stay recoverable for N days, beyond the seconds-long undo toast; protects against a fat-fingered bulk delete (audio is unlinked permanently today once the toast lapses).
- 🔬 **Backup *restore*** — `phoneme import <backup.zip>` to complement the existing export (export with no restore is a half-feature). **Note:** DPAPI-encrypted provider keys are *not* portable across machines (decrypt silently fails → treated as unset), so a machine migration loses all keys — document this regardless. *(The "Not convinced yet" backup-ZIP line reasoned only about export.)*
- 🔬 **Retention policies per tag / per profile** — finer-grained than the global age/count auto-delete.
- 🔬 **Data-access transparency log** — what hook / AI step touched which recording.

### Teams & cloud
- 🔬 **Phoneme Cloud (optional, self-hostable)** — shared catalogs + role-based access for teams; the desktop daemon stays fully offline-capable.

### Foundation & DX
- 📋 **Playwright E2E UI coverage** — a full suite driving the frontend against the real Rust backend over IPC (after the architecture stabilizes across platforms).
- 🔬 **Command palette** — everything reachable by typing.
- 🔬 **Accessibility pass** — screen-reader support (NVDA/JAWS), ARIA labels, font scaling, high-contrast theme.
- 🔬 **Batch operations** — batch delete / batch tag update over a selection.
- 🔬 **Scale to 100k+ catalogs** — DB vacuum strategy, indexing, list() N+1 trims, quoted-FTS5 phrase + boolean search. *(The vector-ANN half is promoted to H1 as a moat prerequisite; this is the rest.)*
- 🔬 **Data-integrity hygiene** — a single `schema_version` column instead of per-feature `*_migrated` booleans, and the missing `ON DELETE CASCADE` on `dismissed_speaker_suggestions` (its rows orphan on recording delete).
- 🔬 **Internal tidy-ups** — `catalog.rs` is now split into per-domain modules (`catalog/`); the remaining god-files to split are `config.rs` (~5.5k) and `recorder.rs`. Plus `cargo-deny` + coverage in CI and redundant-clone perf trims. *(Front-load `keyboard.ts` / `RecordingsView/index.ts` — nav bugs measurably recur there, so they earn a split sooner than the Rust ones.)*

*(Protocol versioning moved up to Horizon 0 — it's cheap insurance that gets harder to retrofit with each added client surface.)*

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
| Backup/restore ZIP (as a headline product feature) | Manual export covers most of it; the DB is a single copyable file. *(But the missing `import <backup.zip>` **restore** half — and the fact that DPAPI keys don't survive a machine move — is now a 🔬 item under Horizon 3 → Data & privacy.)* |
| Azure Speech / AWS Transcribe | Enterprise pricing; not the target user; add only on demand |
| Portable (unsigned) ZIP | A CI task, not a product feature |
| Winget / Scoop packages | Same — packaging automation, not a roadmap item |
| Meeting-app awareness (auto-detect Zoom/Teams) | Brittle, false-positive-prone, and surveillance-y for a privacy-first app |
| Voice commands / wake word | Push-to-talk already solves hands-free; wake-word is a false-trigger rabbit hole |
| Transcript git-style version graph | YAGNI at this scale; original-vs-current diff covers ~95% |
| Acoustic echo cancellation (speaker→mic bleed) | Genuinely hard research problem; honest answer is "wear headphones" |
| Word-by-word streaming transcription | The bounded live preview covers the need; full real-time captioning is a v2.0-grade rabbit hole |
