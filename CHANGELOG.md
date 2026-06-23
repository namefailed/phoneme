# 📦 Phoneme Changelog

Shipped releases — what landed in each. **Forward-looking plans live in [`ROADMAP.md`](ROADMAP.md)**; unvetted/parked ideas live in [`docs/IDEAS.md`](docs/IDEAS.md).

---

## 🚧 v1.8.x — Recall, Meetings & Hardening (in development)

*Workspace version `1.8.1`. Closing promise-vs-reality gaps and hardening the
trust boundary.*

### Library & organization

- [x] **Pinned recordings** — pin a recording so it sorts to the **top of the
  library**, independent of favorites. A 📌 toggle in the list row and the detail
  header, a **Pinned** Library sidebar filter (with its own count badge), and a
  pinned-first sort applied in SQL (`pinned DESC` leads the ORDER BY, ahead of the
  date sort, so pins float to the top regardless of sort direction). Backed by a
  new `recordings.pinned` column on the `Recording` DTO, a `SetPinned` IPC (plus
  `pinned` on the list filter), `phoneme edit <ID> --pin/--unpin`, and matching
  REST (`POST /api/recordings/{id}/pinned`) and MCP (`set_pinned`) surfaces. Pins
  survive restarts and travel with library exports.
- [x] **Show/hide the Favorites & Pinned quick-action columns** — Settings →
  Interface → Library layout gains two toggles that hide the ⭐ Favorites and 📌
  Pinned columns **and** their Library sidebar sections together, in one switch
  (per-device, stored in `localStorage`, default off). The list and sidebar redraw
  live the moment you flip them — no reload, no daemon round-trip.
- [x] **Browse by entity (cross-recording entity facet)** — the extracted
  entities that were only ever per-recording chips are now a **library-wide
  browse + filter**, exactly mirroring the tag facet. A new **Entities** sidebar
  section groups every distinct extracted entity by kind (People / Organizations
  / Topics / Terms), each value a clickable row showing how many recordings
  mention it; clicking one filters the library to those recordings (toggle off by
  re-clicking, or via a dismissable pill in the search header). Backed by a new
  `ListAllEntities` IPC (returning `{kind, value, count}` rows from
  `Catalog::entity_facets`) and an `entity_value` / `entity_kind` filter on the
  list query (a `recordings.id IN (SELECT … FROM entities …)` subquery, the entity
  counterpart of `tag_id`), with a matching `phoneme entities [--kind]` CLI.
- [x] **Tasks / reminders from voice** — an LLM enrichment step that pulls
  **task-shaped action items** (`{text, due_hint?}`) out of a transcript into a
  per-recording `tasks` child table, browsable and **checkable**. The detail pane
  gains a **✅ Tasks** section: a checkbox per action item (strike-through + dim
  when done, an optional `due_hint` shown as a muted suffix), plus a **✅ Extract**
  button that runs the step on demand. The sidebar gains a **Tasks** section with
  **Open** / **All tasks** filter rows. Re-extraction **preserves any task you
  already checked off** (the `done` flag survives when a task's text recurs), and
  an empty extraction never wipes the existing list. Backed by a new `tasks` table
  + `tasks_model` column (migration `20260622000010_add_tasks.sql`), a `tasks`
  Playbook enrichment target (opt-in, no `[tasks]` config section), `SuggestTasks`
  / `SetTaskDone` / `ListAllTasks` IPC + `tasks_updated` / `tasks_failed` events +
  a `task_state` (`has_open` / `has_tasks`) list filter, and a
  `phoneme suggest-tasks <ID>` / `phoneme tasks [--open] [done|undone …]` CLI.
  `due_hint` is the model's deadline phrase stored verbatim — Phoneme does **not**
  parse it to a date or schedule a reminder.
- [x] **Task management — edit, not just check** — the per-recording ✅ Tasks
  section becomes a real list manager: **add** a task by hand, **edit** its text
  inline (double-click or ✎), **delete** it (✕), **reorder** by drag, and **hide
  done** with one toggle. Hand-added / edited tasks are `source='manual'` and
  survive a re-extraction (a new `source` + `sort_order` column on `tasks`); the
  AI only ever replaces its own `'llm'` rows. A new **📋 All tasks** modal (the
  sidebar Tasks section's **View all…** row) is the cross-recording "everything I
  have to do" list — every task in one place, checkable in line, filterable, with
  a click-through pill to each task's recording. Backed by `AddTask` / `UpdateTask`
  / `DeleteTask` / `ReorderTasks` IPC over the existing `ListAllTasks` read, with
  matching `phoneme tasks add | edit | delete | reorder` CLI (full GUI ↔ CLI ↔ IPC
  parity — `edit` preserves the due hint unless you pass `--due` / `--clear-due`).
- [x] **Entity management + library-wide merge** — extracted entities become
  editable, not just viewable. In the detail pane's 🔎 Entities section you can
  **+ Add** an entity (pick its kind), fix one inline (double-click), or delete it
  (✕ on hover). A new **Entity manager** modal (the section's **Manage** button)
  curates the whole library at once: **rename** a value everywhere, or tick two or
  more variants of one kind and **merge** them into a single canonical value
  (e.g. "ACME" / "acme corp" → "Acme Corp"). Like tasks, hand-curated entities are
  `source='manual'` (new column) and survive re-extraction. Backed by `AddEntity`
  / `UpdateEntity` / `DeleteEntity` / `MergeEntities` IPC and an `entities_merged`
  event, with matching `phoneme entities add | edit | delete | merge` CLI (full
  GUI ↔ CLI ↔ IPC parity).

### Transcripts

- [x] **Daily / weekly digest (period digest)** — generate one LLM **rollup
  across every recording in a date window** (what was discussed, decisions
  reached, open/action items), distinct from the per-recording summary and the
  meeting-scoped digest. The daemon selects the window (`since`..`until`,
  oldest-first), concatenates each recording's transcript prefixed with its date
  + title (capped to a size limit so a huge week can't overflow the model's
  context), and runs the merged text through the configured `[summary]` provider.
  Re-running the same window upserts in place (stored in a new `period_digests`
  table keyed by the range). New `phoneme digest` CLI (`--daily` default,
  `--weekly`, explicit `--since/--until`, `--model`, and `--show` to read the
  stored digest), `rerun_period_digest` / `get_period_digest` /
  `list_period_digests` IPC requests, `period_digest_updated` /
  `period_digest_failed` events, and Tauri commands behind them. Period digests
  travel with library exports. (Scheduling the rollup to fire automatically is a
  deliberate follow-up.)
- [x] **Topic timelines (auto-chapters)** — an LLM enrichment that splits a
  recording into **time-ranged chapters** (a titled, optionally-summarized span
  per topic), so a long recording becomes navigable instead of one wall of text.
  The daemon sends the transcript to the provider on the recipe's `chapters`
  entry, parses the returned chapters, and **snaps each boundary to a real segment
  start** so a chapter always lines up with the audio. Stored wholesale in a new
  `chapters` child table (`start_ms`, `end_ms`, `title`, `summary`) + a
  `chapters_model` provenance column (migration `20260622000000_add_chapters.sql`),
  surfaced as a **Chapters** view in the detail pane (with a ✨ Generate button)
  and an opt-in `chapters` Playbook enrichment target. New `SuggestChapters` /
  `GetChapters` IPC + `chapters_updated` / `chapters_failed` events, and a
  `phoneme chapters <ID> [--show]` CLI. Re-deriving is one re-run away; deleting a
  recording deletes its chapters with it.
- [x] **Ask my archive (local RAG)** — ask a plain-language question and get an
  answer drawn **only** from your own recordings, with a citation for every
  claim. The daemon embeds the question, retrieves the best-matching transcript
  chunks through the *same* hybrid (vector + FTS5/RRF) retriever the search bar
  uses, and streams the answer through your configured `[llm_post_process]`
  provider — no new model, no new index, nothing leaves your machine beyond the
  LLM call you already use for cleanup. Each `[n]` marker in the answer links to
  the recording it came from. New `phoneme ask "…"` CLI (with `--top-k`,
  `--tag/--status/--kind` scoping, and `--json`), a 💬 chat panel in the app, and
  an `ask` IPC request that streams over a new `ask_activity` event. When nothing
  matches it says so instead of guessing; needs semantic search enabled and an
  LLM provider configured.
- [x] **Confidence-driven re-do** — Phoneme now aggregates the per-word ASR
  confidence it already captures into one **mean confidence** per recording
  (computed when transcription completes — no model re-run — and stored in the new
  nullable `recordings.mean_confidence` column + the `Recording` DTO). A transcript
  whose mean falls below the configurable `[whisper].low_confidence_threshold`
  (default **0.6**) is flagged **low confidence**: an unobtrusive amber badge in the
  library list, a one-click **Improve…** button in the detail action row that opens
  the existing re-transcribe flow (optionally with a larger model), and a **"Low
  confidence"** sidebar filter (server-side via the new `low_confidence_below`
  list filter, so it composes with pagination). Graceful by design: providers that
  return no per-word confidence (the OpenAI/Groq cloud transcription endpoints) and
  recordings made before this existed get a `NULL` aggregate — no badge, never
  flagged, no crash. Set the threshold to `0` to disable flagging.
- [x] **User-editable dictation voice commands** — the spoken editing phrases
  ("new line", "new paragraph", "scratch that") are now a customizable
  phrase → action map under **Settings → Dictation → Voice commands** (and
  `[in_place.voice_commands]` in config). Add your own wording, localize, disable
  individual commands, or turn the whole pass off — across every cleanup mode
  (Fast, Off, and AI cleanup, where a customized map is described to the cleanup
  model too). An empty map keeps the built-in defaults, so existing configs are
  unchanged; an entry with an unknown action is dropped on load with a warning
  rather than failing the config.
- [x] **Per-app tone / register** — dictation can now pick its cleanup **recipe**
  (and so the register the AI rewrites toward — formal for an email client, terse
  for an editor, prose for a doc) by the app focused when you **start** dictating,
  via a new `[in_place.app_recipes]` map (app stem → recipe id) editable under
  **Settings → Capture → Dictation → Per-app tone**. A matched app runs the recipe
  through the full pipeline (so its AI cleanup actually runs — a little slower than
  the fast lane, the same trade as a custom-hotkey recipe). A custom hotkey that
  already names its own recipe **wins**; unlisted (or undetectable) apps fall back
  to today's behavior, and a named-but-deleted recipe degrades to the default
  cleanup. Resolved entirely daemon-side at record start over the existing
  `pending_recipe` path — no new IPC, bridge, or migration. Empty (default) =
  unchanged. Windows-only (foreground-app detection).
- [x] **Dictation text macros (snippets)** — define **trigger → expansion**
  shorthands that expand in dictation output before it's typed: say "my email"
  and your address lands instead, "sig" becomes a whole signature block. Edited
  under **Settings → Capture → Dictation → Text macros** (and
  `[in_place.snippets]` in config). Matching is case-insensitive and word-boundary
  aware (so "sig" never fires inside "signal"), longer triggers win over shorter
  ones they contain, and an expansion is never re-scanned (no cascade). It runs in
  every cleanup mode, after polish, so a macro's literal text is never reshaped by
  the rule polish or sent to the cleanup LLM. Unlike voice commands there's **no
  built-in set** — an empty map means no expansion, so existing configs are
  unchanged byte-for-byte; a master `snippets_enabled` switch turns the whole pass
  off without clearing your macros.
- [x] **Dictation history / re-grab (opt-in)** — keep a short, bounded record of
  recent in-place dictations (the **text as typed**, no audio) so a past one can be
  **re-inserted** or **re-copied** — for when a dictation went into the wrong
  window or an app ate the paste. Off by default (`[in_place].keep_history`),
  bounded to the newest 50, and it covers even **ephemeral** dictations (which
  otherwise leave no row). Managed under **Settings → Dictation → Dictation
  history** (Copy + Re-insert at cursor + per-row remove + Clear all), reachable
  with the **`g H`** chord, and scriptable via `phoneme dictation
  history`/`regrab`/`forget`/`clear`. New `ListDictationHistory` /
  `RegrabDictation` / `DeleteDictationHistory` / `ClearDictationHistory` IPC
  requests; re-grab is classified **not retry-safe** so a dropped reply can never
  type the text twice. Re-insert types at the *current* caret (the original window
  is gone). Privacy: the typed text is retained until cleared, so it stays opt-in.
- [x] **Library-wide find & replace** — `phoneme find-replace --library <FIND>
  <REPLACE>` (and the new `find_replace_library` IPC request) runs the same
  literal, revertible replacement across **every** recording's transcript in one
  shot. Recordings with no match are left untouched (no version churn, no event),
  and the timing layers are re-flowed + re-embedded per changed recording exactly
  like a single-recording edit.
- [x] **Compounding Playbook steps** — a recipe's `transform` steps chain by
  default (each refines the previous, toward a "perfect" transcript), with a new
  per-step `input = previous | base` to instead run an independent pass off the raw
  transcription. Every step's output is recorded as an inspectable, revertible
  **version**: Compare-versions shows the full chain and a "Revert to this version"
  restores it (re-flowing timing + re-embedding).
- [x] **Timeline matches the cleaned transcript** — the Timeline/Synced views used
  to keep the raw machine timing even after cleanup rewrote the text, so they
  disagreed with the panel. Cleanup now re-aligns the words onto the cleaned text
  into a separate "cleaned" timing layer (the raw machine truth is preserved), and
  both views gained a **Raw ⇄ Cleaned** toggle.
- [x] **Entity extraction** — a real Playbook **Enrichment** step (target
  `entities`) that pulls **structured, typed entities** (`person` / `org` /
  `topic` / `term`) out of a transcript with an LLM — richer than the flat
  auto-tags. Stored in a new `entities` child table (with the per-recording
  `entities_model` provenance, mirroring `summary_model`), surfaced as typed chips
  grouped by kind in the detail pane with a 🔎 **Extract** button, and available on
  demand via `SuggestEntities` / `phoneme suggest-entities <ID>`. Add the built-in
  `entities` entry to a recipe to run it automatically, or run it ad hoc; a re-run
  replaces the set. Reuses the existing auto-tag/summary LLM machinery (same
  provider resolution, streaming, skip/empty/error handling); the default pipeline
  is unchanged unless the recipe includes the step.
- [x] **Summaries stream live in the detail pane** — the AI summary peek and the
  whole-meeting digest card now render the LLM output **token by token as it
  generates**, instead of sitting on a "Generating summary…" placeholder until the
  final text lands. They subscribe to the daemon's existing `llm_activity` stream
  (the same one the 🧠 AI-activity popout consumes) for the `summarizing` stage and
  paint the accumulating text with a live spinner; the poll / `summary_updated` /
  `meeting_digest_updated` settle step still overwrites the (capped) live view with
  the full stored summary, so the final shown text is always authoritative.
  Frontend-only — the daemon already streamed every LLM stage.

### Reliability & foundation

- [x] **Saved searches honor the Low-confidence filter** — a saved search captured
  with the **Low confidence** filter on now actually filters server-side when run
  (`phoneme list --saved` / the Saved-searches menu). The saved-filter mirror was
  missing the toggle, so it previously ran unfiltered; it now maps to the
  configured `[whisper].low_confidence_threshold` like the live sidebar filter.
- [x] **Library find & replace reports failures** — `phoneme find-replace --library`
  now warns when some recordings errored mid-sweep (a non-zero `failed` count in
  the IPC reply) instead of quietly reporting a smaller success count. A recording
  with no transcript is still a benign skip, not a failure.
- [x] **Per-binding dictation/preview model overrides actually load** — a custom
  hotkey's one-job whisper model override used to be written only to the main
  server's override slot, which the dedicated live-preview and dictation servers
  never read, so the override was silently dropped whenever in-place dictation ran
  on its own (or the preview) bundled server. Each bundled server now has its own
  override slot, and in-place dictation publishes to the slot of the server it
  actually dials (and waits for that server to respawn on the new model). Also
  closed a related gap where an auto-materialized preview server and a dictation
  server pinned to `main + 1` could collide on the same port.
- [x] **IPC wire protocol version + handshake** — the daemon↔client NDJSON wire
  now carries an explicit `PROTOCOL_VERSION`, and a new `Handshake` request lets a
  client check it on connect. A `phoneme` CLI built against a breaking wire
  revision now refuses to run against an incompatible daemon with a clear
  "run `phoneme daemon restart`" message instead of failing obscurely later.
  Best-effort + backward-compatible: an older daemon that predates the handshake
  is treated as unversioned and the client proceeds on the additive contract.
- [x] **No `unwrap()` on a runtime path** — every production `unwrap()` now either
  documents why it can't fail with `.expect("…")` or sits behind a guard that
  proves it safe (mutex locks, `chunks_exact(4)` conversions, `Option` access after
  an `is_none()` check, static regexes). A CI step runs `clippy::unwrap_used`
  against the non-test build so a new panicking `unwrap()` can't slip into a
  critical task and take the daemon down.
- [x] **One source of truth for per-step LLM config** — Doctor's connection probe
  and the pipeline's summary/title/tag/entry steps used to re-derive provider
  inheritance independently, so Doctor could test a different endpoint than the
  pipeline actually used. Both now resolve through one `LlmPostProcessConfig::resolve_step`
  helper, so the probe and the real call can't drift.
- [x] **Foreign-key integrity on voiceprint links** — `forgotten_voiceprint_links`
  now carries an `ON DELETE CASCADE` foreign key to `named_voiceprints` (rebuilt
  via migration, orphans filtered), so a forgotten-voice row can't outlive the
  voice it points at.
- [x] **`load_config` is a pure read** — loading config no longer mutates
  `config.toml` on disk or injects the preview model as a side effect. The read is
  pure; an explicit `reconcile_and_persist_config` step does the migrate-and-write
  and `apply_runtime_defaults` does the in-memory preview wiring, each called
  deliberately at startup.
- [x] **Meeting digests survive a backup round-trip** — a whole-meeting digest
  lives in its own side table keyed by `meeting_id` (a meeting isn't a single
  recording row), so it was neither exported nor restored and silently vanished on
  `export` → `import-backup`. The library backup now captures every digest (a new
  `ListMeetingDigests` read, fetched alongside the tag list) and replays it on
  restore via the idempotent digest upsert, so a re-import never clobbers a digest
  regenerated since. Pre-digest backups still restore cleanly.
- [x] **Deleting every track of a meeting clears its digest** — removing a meeting
  track-by-track (rather than via *Delete session*) left its digest row orphaned,
  since the digest table has no foreign key to cascade. Deleting the last track of
  a meeting now drops the digest too (best-effort; never fails the delete).
- [x] **Auto meeting-digest no longer stalls the queue** — the on-finalize
  meeting digest ran inline on the single serial transcription worker, blocking the
  next recording for the whole LLM call. It now spawns off-thread like the
  on-demand re-run, so the queue keeps draining while the digest is written.
- [x] **Entity extraction provenance + partial parses** — the per-recording
  entities step now records its model only after a non-empty parse, so the
  provenance line can't name a model that produced nothing while the displayed
  entities came from an earlier run; and a single malformed object in the model's
  JSON reply no longer discards every entity — the array is parsed element-by-
  element, keeping all the well-formed ones.
- [x] **Opt-in, local-only diagnostics bundle** — a **Doctor → Export diagnostics**
  button (and the `ExportDiagnostics` IPC behind it) writes one sanitized JSON
  snapshot you can attach to a bug report. It carries exactly three things:
  app + OS info, the **masked** config (every API key / secret redacted through
  the same `secrets::mask_json` the GUI and CLI redactors use — never a plaintext
  key), and a bounded tail of the daemon log (400 lines by default, hard-capped).
  No audio, no transcripts, no catalog contents, and **no network** — the daemon
  assembles it from disk + in-memory config and writes it to
  `<data_dir>/diagnostics/phoneme-diagnostics-<timestamp>.json`, returning the path
  to reveal. "No telemetry" stays true; this just means a field crash is no longer
  invisible. A masking round-trip test asserts no secret can leak into the bundle.
- [x] **No more hook double-fire** — the daemon's legacy `hook.commands` loop is now
  gated behind `schema_version < CURRENT_SCHEMA_VERSION`, so it runs **only** for a
  genuinely un-migrated config. Before, a stale or hand-edited config that already
  carried a migrated schema version (with recipe-driven Hook entries) *and* still
  had non-empty `hook.commands` could fire a shell hook **twice** — once from the
  legacy loop, once from the recipe's `run_hook_steps`. Migration empties
  `hook.commands` and is idempotent, so the gate closes that window while still
  honoring legacy hooks for an old config that hasn't migrated yet. A regression
  test (`stale_legacy_hook_does_not_double_fire_post_migration`) pins the
  single-fire invariant.
- [x] **CI hardening** — the `cargo audit` and `pnpm audit` jobs are now **blocking**
  rather than advisory: `continue-on-error` is gone, so a new vulnerability fails
  the build instead of scrolling past in green (handle a real advisory with a
  scoped `--ignore RUSTSEC-…` + a reason, never by muting the whole job). The
  workspace test job also dropped `--test-threads=1` and runs `cargo test
  --workspace` in parallel now that per-test DB isolation (in-memory + per-test
  tempdir) exists — so CI catches isolation rot instead of masking it behind serial
  execution.

### Providers & models

- [x] **Spoken-language detection → routing** — Phoneme now stores the language
  the transcriber *detects* for each recording (a 🌐 badge in the detail pane) and
  can **route by it**: a new **Settings → Transcription → Language routing** table
  (and `[[language_routes]]` in config) maps each detected language to an optional
  Whisper-model override and an optional cleanup recipe. It's two-pass and opt-in
  — the first transcription auto-detects; a rule that names a *different* model
  re-transcribes that one recording under it (reusing the safe per-job model-swap
  path, so the #49 server-thrash safety holds), while a recipe-only rule just
  routes post-processing with no extra STT pass. A per-keybind model/recipe
  override always wins; an empty table is exactly today's behaviour. Detection
  rides the providers that report a language (local `whisper.cpp` `verbose_json`,
  Deepgram `detect_language`, AssemblyAI) and degrades to "no detection, no route"
  on the ones that don't (the `gpt-4o-transcribe` family, the native in-process
  engine). Backed by a new nullable `recordings.detected_language` column and a
  `language: Option<String>` on the core `Transcription`.

- [x] **Manage your local Ollama models from inside the app** — a new **Manage
  local models** surface (a button on the Models picker's Post-processing tab and
  in **Settings → Post-Processing**, both shown when your Cleanup provider is a
  local Ollama) lists the models installed in Ollama with their on-disk size,
  pulls a new one with a live progress bar, and deletes ones you no longer use
  (behind a confirm). Previously the only in-app pull was buried in the first-run
  wizard; now you can grow or shrink your local model set any time. Backed by the
  tray's `ollama_list_installed` / `ollama_pull_model` / `ollama_delete_model`
  commands talking to the local Ollama HTTP API (`/api/tags`, `/api/pull`,
  `/api/delete`) — the same plane the wizard's pull already used, so there's no
  daemon round-trip for a purely-local concern.

### Internals & refactors

- [x] **Unified config `schema_version`** — the per-feature one-time-migration
  latches (`playbook_migrated`, `hooks_migrated`) are replaced by a single
  top-level `schema_version` integer. Each migration owns a version step (the
  Playbook reconcile is `0 → 1`, the legacy `[hook]` cutover is `1 → 2`); on load
  Phoneme runs only the steps newer than the stored version and then records the
  current version, so every migration runs **exactly once** and re-loading an
  already-migrated config does nothing. Existing configs upgrade transparently:
  the old booleans still deserialize and are read **once** to infer the correct
  starting version (so a config that already migrated is never re-migrated), after
  which they are dropped from `config.toml` on the next save. Future structural
  migrations just claim the next version step instead of inventing a new flag.
- [x] **`catalog.rs` split into a module directory** — the 7.6k-line catalog
  god-object became `catalog/` with per-domain files (`recordings`, `embeddings`,
  `tags`, `speakers`, `segments`, `saved_search`, plus `mod.rs` for the structs and
  shared helpers). A pure move: the public API and behavior are byte-for-byte the
  same, verified by a method-set diff.
- [x] **Incremental embedding cache** — upserting or deleting one recording's
  vectors now patches that recording in the in-memory corpus (copy-on-write, with a
  generation guard against lost updates) instead of dropping the whole 300 MB
  snapshot and re-decoding the entire library from SQLite on the next query.
- [x] **Semantic search stops nerfing FTS5** — the query sanitizer now quotes and
  escapes each term instead of stripping punctuation and force-appending `*` to
  every word. Phrases (`"exact match"`), hyphenated terms, and code tokens survive;
  bare words still get prefix matching.
- [x] **Linear-time `replace_ignore_case`** — the case-insensitive find/replace
  behind speaker renames is now a single Unicode-safe regex pass instead of an
  O(N·M) hand-rolled scan.
- [x] **Overlay frontend split into components** — the 750-line `overlay.ts` became
  a thin wiring index over focused modules (`waveform`, `drag`, `captions`,
  `shape`), each unit-testable on its own.

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
- [x] **Filtered meaning-search** — a semantic search can be scoped by the same
  Library filters as the list (tag, status, date, kind, favorite, in-place,
  tag-presence): `catalog::hybrid_search` takes an optional `ListFilter` that
  restricts the candidate set after ranking, before the limit, so the top
  in-scope results come back. `SemanticSearch` IPC gained an optional `filter`
  (additive; unscoped by default); CLI parity via `phoneme search --tag/--status/
  --kind` (and through `phoneme list --semantic`).
- [x] **Run saved searches server-side** — a saved search can now be executed by
  id: `run_saved_search` (`RunSavedSearch` IPC, `catalog::run_saved_search`)
  parses the stored `filter_json` into a `ListFilter` and runs the list query in
  the daemon, returning the same recordings shape as `list_recordings`.
  Malformed filter JSON reports a clear error instead of silently running the
  whole library. `phoneme list --saved [id]` runs (or lists) them.
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

- [x] **Whole-meeting digest** — one AI digest synthesized across **all** of a
  meeting's tracks (mic + system together), distinct from the per-recording
  summary. The daemon assembles the merged meeting transcript (every track,
  source-labelled) and runs it through the **configured summary provider** — no
  new provider keys — storing the result keyed by `meeting_id` in a new
  `meeting_digests` table. It generates automatically when a meeting finalizes
  (after both tracks transcribe), gated on the same `[summary].auto` switch as
  the per-recording auto-summary, and on demand via a ✨ digest card in the merged
  meeting view, `phoneme meeting digest <meeting_id> [--model …]`, or the
  `RerunMeetingDigest` IPC. Surfaced over `GetMeetingDigest`, with
  `meeting_digest_updated` / `meeting_digest_failed` events mirroring
  `summary_updated` / `summary_failed` at meeting scope.
- [x] **Meeting templates** — the whole-meeting digest is now a **selectable
  recipe**, not one hardcoded prompt. A Playbook recipe carries a `scope`
  (`recording` — the default for every existing recipe, so nothing changes — or
  `meeting`); a `scope = meeting` recipe (a "meeting template") runs **once** over
  the merged meeting transcript. Seeds ship a built-in `meeting_digest` plus
  `standup` and `interview` example templates (differing only by prompt). Pick the
  one the auto- and on-demand digest uses with the new top-level
  `meeting_recipe_id` config key (empty = the built-in digest, identical to prior
  behaviour — no migration), the **Meeting template** selector in Settings →
  Playbook, or per-digest from the merged view's picker / `phoneme meeting digest
  <id> --template <recipe_id>` / `RerunMeetingDigest.recipe_id` (a one-shot, never
  persisted). The auto-fire keeps its exactly-once `digest_in_flight` claim.
- [x] **Merged meeting view** — selecting a meeting's group header opens a single,
  read-only reading of every track, labelled 🎤 Microphone / 🔊 System audio with the
  diarizer's `[Speaker N]` turns surfaced, plus Copy / Export
  (`MergedConversationDetail.ts`, `mergeMeeting.ts`). Now a **chronologically
  interleaved** chat timeline (`mergeChronological()`, ordered by per-track segment
  offsets), falling back to the coarse source-sectioned merge only when segment
  timings are absent.
- [x] **Speaker-diarization provider picker** — Settings → Transcription now exposes a
  Speaker Diarization section to choose who-spoke-when: off, **Local** (speakrs ONNX),
  **Deepgram**, **AssemblyAI**, or **ElevenLabs** (`SectionDiarization.ts`). Cloud diarization rides the
  same provider's transcription API, so the section shows a live warning when the chosen
  diarization provider can't run with the configured transcription backend (e.g. Deepgram
  diarization picked while Local transcribes) instead of silently doing nothing.
- [x] **ElevenLabs Scribe at full parity** — the ElevenLabs transcription provider now
  returns everything the others do: a **per-word timeline** (powering the Synced view and
  transcript↔waveform word seek), the **detected language**, and **speaker diarization**
  (`[Speaker N]` turns from Scribe's per-word `speaker_id`, gated on the new `elevenlabs`
  diarization backend) — previously it surfaced only flat text. Words + language are
  unconditional; diarization rides the Scribe call like Deepgram/AssemblyAI. The
  word→turn assembly is unit-tested. *(We support everyone we can — no provider left a
  second-class citizen.)*
- [x] **Named-speaker recognition** — the local diarizer now captures a voiceprint
  (centroid embedding) per speaker; naming a speaker enrolls that voice into a
  cross-recording library, and opening a later recording suggests known voices for
  still-unnamed speakers ("Sounds like Alex? ✓ / ✗") in the Rename-speakers modal.
  On-demand, so a voice named *after* a recording was transcribed is still suggested
  on it. `[diarization].recognize_speakers` toggles it; `voiceprint_match_threshold`
  tunes the cosine bar. Local diarization only.
- [x] **DER eval harness (dev)** — a pure, unit-tested collar-0 Diarization Error
  Rate metric (`phoneme_core::der`: `parse_rttm`, `compute_der`, missed /
  false-alarm / confusion with overlap-based speaker mapping), plus an `#[ignore]`d
  harness that runs the local diarizer on an audio fixture and scores it against a
  reference RTTM — for measuring diarizer quality / catching regressions (an
  optional nightly check, never a PR gate).
- [x] **Voiceprint EER calibration harness (dev)** — a pure, unit-tested metric
  (`phoneme_core::voiceprint_eval`: `calibrate`, `trial_scores`, `compute_eer`)
  that turns a set of labelled voiceprints into the verification calibration
  curve. Genuine (same-speaker) and impostor (different-speaker) vector pairs are
  scored with the recognizer's own `voiceprint::cosine_similarity`, then a
  threshold sweep gives FAR/FRR at each point and the interpolated **equal error
  rate** + its threshold. Gives `voiceprint_match_threshold` (~0.5, eyeballed) a
  measured basis. Not wired into the pipeline — an eval harness like the DER one.
- [x] **WER / CER accuracy harness (dev)** — `phoneme_core::wer` adds the
  headline ASR accuracy metric: `WER = (substitutions + insertions + deletions)
  / reference_word_count`, scored with Levenshtein edit distance. `compute_wer`
  works at the word level; `compute_cer` at the character level. Both normalize
  the input (lowercase + strip ASCII punctuation) so surface formatting doesn't
  inflate the score. Returns a `WerReport` with the full S/I/D breakdown and
  `ref_units`; `wer` is `None` when the reference is empty (denominator
  undefined). WER can exceed 1.0 when the hypothesis has many extra words — not
  capped. Pure, dependency-free, thoroughly unit-tested; not wired into the live
  pipeline.
- [x] **Cohort score normalization for speaker matching (S-norm / AS-norm)** —
  raw cosine sits on a different scale per voice (some speakers are "closer" to
  the whole cohort than others), so one global threshold over-accepts central
  voices and over-rejects outliers. `voiceprint::best_match_normalized` /
  `normalized_score` z-score each comparison against the *other* enrolled voices
  (the cohort is the candidate set itself): `s_norm` normalizes the probe side,
  `as_norm` is symmetric (probe- and target-side averaged). Opt-in behind
  `[diarization].voiceprint_score_norm` (`off` \| `s_norm` \| `as_norm`, default
  **off** = byte-for-byte the old raw path), with its own z-score bar
  `voiceprint_score_norm_threshold` (default 2.0). `recognize_speakers_for`
  routes on the flag; a cohort of one degrades gracefully to the raw score (no
  NaN/divide-by-zero). On a constructed uneven-spread case the normalized EER
  (via the harness above) is strictly below the raw EER. Local diarization only;
  config-only for now (no Settings toggle yet) and per-speaker thresholds left as
  a follow-up (S-norm already re-centers per probe).
- [x] **Duration-weighted named-voice centroid** — a named voice's cached centroid
  now weights each enrolled capture by how long that speaker actually spoke, so a
  long, clean sample outvotes a brief one-word blip instead of counting the same
  (`voiceprint::weighted_mean_centroid`). Weighting is applied *after* the existing
  outlier rejection, on the survivors only — length can't buy a wrong-speaker
  capture into the template. Each capture stores a `duration_ms` (migration
  `20260620000000`; summed from that speaker's segment spans at transcribe time);
  legacy captures default to `0`, which falls back to equal weighting, so a library
  built before this recomputes to the identical centroid until new captures arrive.
  Local diarization only.
- [x] **Name propagation + reversible forget (V5, backend)** — naming a speaker can
  now back-fill that name onto the **same unnamed voice in past recordings**.
  `catalog::propagation_candidates` scans every unnamed, un-display-named capture in
  other recordings and scores it against the named voice with the recognizer's own
  scorer (score-norm aware), at/above `voiceprint_match_threshold`;
  `apply_propagation` then applies each like a normal naming (display name +
  enrollment), **never overwriting an already-named speaker** and idempotent on
  re-run. Routed by `[diarization].name_propagation` (`ask` \| `auto` \| `off`,
  **default `ask`** = surface candidates, change no past recording automatically;
  `auto` back-fills and reports the count; `off` no-ops). The `SetSpeakerName`
  response now carries a `propagation` block (policy + applied count + candidates).
  **Forget is now reversible** — `forget_named_voice` soft-deletes (new `deleted_at`
  column + an undo log of the captures it unlinked; migration `20260620000001`)
  instead of hard-deleting, every read path filters `deleted_at IS NULL`, and a new
  `undo_forget` / `UndoForgetNamedVoice` IPC restores the voice and re-links its
  captures (skipping any re-enrolled onto another voice since). The confirm prompt +
  "don't ask again" toggle, and full **merge**-undo (needs a move-log), are
  follow-ups. Local diarization only.
- [x] **Custom local diarization models** — `[diarization].models_dir` points the local
  diarizer at a folder holding your own speakrs bundle (segmentation + embedding ONNX),
  loaded via `OwnedDiarizationPipeline::from_dir` instead of the pretrained download;
  empty keeps the defaults. Settings field under Diarization; the cache reloads on
  change. (The dead `local_model_path` key it replaces was never wired in.)
- [x] **Track-aware Meeting Mode** — a meeting's **mic track** is now labelled as one
  speaker, **You**, without running the diarizer at all; only the system/loopback track
  is diarized. The mic track is a single voice (yours), so diarizing it only burned time
  and produced spurious multi-speaker labels on one person. This halves a meeting's
  diarizer work and gives the mic track a clean, single-speaker transcript. The label
  reuses the canonical `[Speaker N]` machinery (a `speaker_names` row names label 1 → You),
  so it stays user-renamable and the merged-meeting view is unchanged. Normal single
  recordings and the system track are completely unaffected.
- [x] **In-recording speaker correction** — fix the diarizer's per-segment assignments
  after the fact: **reassign** one segment to another speaker, **merge** two speakers
  into one, or **split** some segments off onto a fresh speaker (`catalog::reassign_segment`
  / `merge_speakers` / `split_speaker`). `transcript_segments` stays authoritative (the
  timeline / Synced views re-derive from it) and each op rebuilds the prose transcript's
  `[Speaker N]:` markers — and the per-word layer — in one transaction, so the change shows
  everywhere at once. On a **merge** the surviving speaker keeps its name (adopts the other's
  only when unnamed) and the merged-away speaker's voiceprint is dropped (the centroid is
  per-label; a re-transcribe re-captures the merged label) with any affected named voice
  recomputed; a split-off speaker has no name/voiceprint until enrolled. New
  `ReassignSegmentSpeaker` / `MergeSpeakers` / `SplitSpeaker` IPC (mutating, not retry-safe,
  emit `speaker_name_updated`) + Tauri commands, and CLI `phoneme speaker reassign|merge|split`.
- [x] **In-recording speaker correction — in the app** — the correction backend above is now
  wired into the UI. In the **merged meeting view**, every chronological turn has a small **⋯**
  button beside its speaker chip that opens a keyboard-accessible, Esc-closable popover to
  **reassign** that turn to another speaker, **split** it onto a brand-new speaker, or **merge**
  that speaker into another (each scoped to the right track — the turn's segment indices are
  resolved from the loaded timeline, so a clicked turn maps to the exact `reassign`/`split`
  segments). The single-recording **Rename speakers** modal gains a per-speaker **Merge into…**
  picker. Every op rides the new `ipc.ts` wrappers (`reassignSegmentSpeaker` / `mergeSpeakers` /
  `splitSpeaker`) and refreshes off the daemon's `speaker_name_updated` event, the same reload
  path as a rename.
- [x] **Expected-speakers prior** — `[diarization].expected_speakers = n` tells the local
  diarizer how many voices to expect. If it still finds MORE than `n` (one voice over-split
  into several), Phoneme greedily merges the closest clusters by voiceprint cosine until exactly
  `n` remain; finding `n` or fewer is left untouched (it never splits). speakrs has no native
  target-count knob — it clusters by similarity threshold only — so this rides as a
  post-clustering merge (`merge_to_expected_count` in `diarization.rs`, sharing the matrix-fold
  /relabel path with the automatic voiceprint merge). Unset (or `0`) keeps today's behavior;
  cloud providers ignore it. Local diarization only.

### CLI

- [x] **`phoneme import-backup <ZIP>`** — restore a library backup, the inverse of
  `phoneme export <FILE>`. Re-inserts every recording from the archive's `catalog.json`
  into the catalog and copies its audio into the audio dir. Idempotent: an id already
  present is skipped (counted, never overwritten), so a re-import never duplicates a row
  or reverts a later hand edit. Shuts a running daemon down first to release `catalog.db`
  (like `doctor --rebuild-catalog`) and reports imported/skipped counts. Core restore +
  the shared backup-zip writer now live in `phoneme-core::backup` (round-trip tested);
  the CLI `export` and `import-backup` are thin wrappers over them.
- [x] **`--recipe <ID|NAME>` on `record` + `retranscribe`** — pick a Playbook recipe
  from the CLI the way the GUI does. Available on `phoneme record` (blocking /
  `--oneshot` / `--duration`), `phoneme record start`, `phoneme record toggle`, and
  `phoneme retranscribe`. The value resolves locally against `[[recipes]]` — by id
  first, then case-insensitively by name — and the resolved id rides the existing
  `recipe_id` IPC field; absent = the default pipeline. A value matching no recipe is
  an error that lists the available recipes (no silent fallback to default).
- [x] **`phoneme find-replace <ID> <FIND> <REPLACE>`** — literal (not regex)
  find-and-replace across a recording's transcript, case-insensitive by default
  (`--case-sensitive` for exact case). Only the live transcript is rewritten — the
  preserved original/clean copies stay, so it's revertible — and the timing layers
  re-flow + the text re-embeds like a hand edit (`FindReplace` IPC,
  `catalog::find_replace_transcript`). A no-match is a no-op; prints/returns the
  replacement count.
- [x] **`phoneme speaker calibrate`** — a read-only command that suggests a
  `voiceprint_match_threshold` from *your own* enrolled voices, so the speaker-
  recognition cutoff has a measured basis instead of the hand-picked `0.5`. It
  scores every same-named-voice pair (genuine) against every different-named-voice
  pair (impostor) with the recognizer's own cosine, finds the equal-error-rate
  (EER) threshold that best separates them, and prints the suggested value beside
  the current one (`--json` for the full report). It only ever suggests — it never
  touches the config — and reports "not enough labelled data" below two named
  voices with two-plus captures each. Wraps the `phoneme_core::voiceprint_eval`
  metric (the dev EER harness) over an opened-read-only catalog; no daemon needed.

### Transcription

- [x] **Custom vocabulary / glossary** — a new **Settings → Transcription → Custom
  vocabulary** field (`[whisper] initial_prompt`) where you list the names, jargon,
  and acronyms the transcriber keeps mis-hearing (e.g. `Phoneme, pyannote, WebView2,
  Namef`). It's sent as the OpenAI `prompt` field to the whisper-family providers —
  the local `whisper.cpp` server, OpenAI, Groq, and Custom OpenAI-compatible
  endpoints — and as `set_initial_prompt` on the in-process native path, biasing
  decoding toward those terms. Empty by default (wire format unchanged); kept short
  since Whisper only conditions on ~the last 224 prompt tokens. Deepgram, AssemblyAI,
  and ElevenLabs ignore it for now (they expose different keyword mechanisms).
- [x] **Import audio straight from a URL** — `phoneme import <http(s)-url>` (e.g. a
  YouTube link) downloads just the audio track with **yt-dlp** into a temp folder,
  then imports it through the normal pipeline; the temp file is removed afterward
  (Phoneme keeps only its decoded copy). A `--format` flag (`m4a` default, or
  `mp3`/`flac`/`wav`) picks the extracted format. Makes it easy to pull real-world
  clips and A/B transcription settings via `retranscribe` + the compare-versions
  view. Auto-detects an installed JS runtime (deno/node/bun) for YouTube's
  extractor. Requires yt-dlp + ffmpeg on PATH, and `phoneme doctor` reports
  whether yt-dlp is available (informational — only URL imports need it).

### Recording

- [x] **Pre-roll no longer eats into the requested length** — a fixed-duration take
  of N seconds now captures a full N seconds of *fresh* audio, with the pre-roll
  lead-in added on top, instead of counting the prepended pre-roll inside the N. The
  `Duration` auto-stop and the `max_duration_secs` ceiling now measure live capture
  only; pre-roll rides above the budget. A take with pre-roll disabled is unchanged.
- [x] **Lost-microphone warning** — if the input device fails mid-recording (mic
  unplugged, driver drop), Phoneme now tells you instead of stopping silently. The
  audio captured before the drop is still saved and transcribed exactly as before;
  on top of that the daemon emits a `DeviceLost` event and the app raises a warning
  toast — "Microphone disconnected — saved the 12.4s captured so far." — so you know
  what happened and that nothing was lost. A normal stop never triggers it.
- [x] **Source column reflects the real capture source** — every recording now
  stores which source it actually used (microphone vs system audio) on its `track`,
  so the list's **Source** column and its hover icon are accurate instead of always
  showing Microphone for single recordings. Pairs with the per-keybind source
  override under Custom Hotkeys.
- [x] **Streaming-type dictation (experimental)** — `[in_place].stream_type`, off
  by default: with Typing delivery, dictated words appear live at your cursor as you
  speak (the streaming preview's committed words, typed only on clean forward
  extensions so the cursor never churns), then a minimal backspace + retype patches
  them up to the accurate final transcript when you stop. The live-preview + final
  batch transcription pipeline is unchanged — this only changes when/how the typed
  fast lane delivers. Settings → Dictation toggle.
- [x] **Steadier live preview** — the live caption no longer reshuffles words you
  already saw. It used to re-transcribe the whole take and replace the caption
  wholesale for the first 15s (i.e. most dictations), so earlier words visibly
  changed as you kept talking. Now it **always stitches** each tick onto the text
  already shown — words once committed are frozen, only the genuinely-new tail is
  appended — with a phase-aware fallback that never blindly re-appends a
  re-transcribed tail (the old duplicated-runs bug). It also advances ~2× more
  smoothly (0.5s min-new gate); weak machines still self-throttle.
- [x] **Tentative tail in the live-preview overlay** — the overlay now dims the
  words it just appended this tick (the freshest, least-settled part of the
  caption, which may still revise) while the stable prefix shown earlier stays
  solid, so you can see which trailing words aren't final yet. The daemon tags
  each `transcription_partial` with `committed_len` (char length of the committed
  prefix) — the boundary it already computes when stitching — and the overlay
  splits the rendered caption there. Both meeting tracks get it in "both" mode.
  Backward compatible: a partial without the field renders all-solid as before.
  Overlay-only; the in-app caption and the final transcript are unchanged.
- [x] **Live preview auto-picks the smallest local model** — when `[preview_whisper]`
  is unset and your final `[whisper]` is a local bundled model, the daemon now runs
  the preview on the **smallest local Whisper model it finds beside the final one**
  (e.g. a `large` final still gets `tiny`/`base` captions) via a dedicated, thread-
  capped second server — automatically, no setup. Resolved once per config (re)load
  and held in-memory only (never written to `config.toml`). Kicks in only when a
  strictly smaller local model exists; cloud/external mains, or a setup whose only
  local model is the final one, fall back to reusing the final provider exactly as
  before. The auto version of the old "Use a dedicated Tiny model" nudge — **final
  transcription accuracy is unchanged** (preview is a separate fast lane). Set
  `[preview_whisper]` to override the pick.
- [x] **Live preview now works during in-place dictation** — dictation previously
  showed no overlay caption at all (the streaming-preview loop was hard-skipped for
  dictation to protect paste latency). It now drives the overlay like any recording,
  with an in-place-scoped teardown that aborts the preview on stop so the typed text
  still lands instantly. Caption duplication is fixed at the source (the rolling-
  window stitch no longer re-appends revised text).
- [x] **Live-preview overlay redesigned as a one-line caption** — a strict single
  line that never wraps or grows: fixed height, **horizontal-resize only** (drag it
  wider/narrower for more/less text, never taller), words reveal one-at-a-time at
  `preview_reveal_words_per_sec` with the newest words kept on the line (older ones
  scroll off the left). The source-swap button now appears only for meetings, and
  the laggy fade in/out is gone.
- [x] **Meeting "both" mode can now stream both tracks concurrently** — a new
  opt-in spawns a **second** live-preview whisper-server so a meeting's mic and
  system tracks caption at the same time instead of alternating on one server.
  Previously "both" mode ran a loop per track but both shared the single
  transcription permit, so only one transcribed per tick (the captions visibly
  lagged at ~half rate); that light, shared-server behavior is still the default.
  Enable **Settings → Transcription → Live Preview → "2nd preview server for
  'both'"** (`recording.meeting_preview_own_server`) to run them concurrently —
  it reuses your dedicated preview model on a derived port (default `5812`) via a
  fourth supervised server (`Config::needed_whisper_servers` /
  `second_preview_needs_own_server`), gated behind "both" mode + a local preview
  model, with **strong warnings** since it keeps a second model resident and runs
  a second concurrent transcription. The overlay now grows to **two lines** in
  "both" mode so the second track's caption is actually visible (it was clipped
  by the one-line window before). Off by default; the weak-box default is
  byte-for-byte unchanged.
- [x] **Smoother meeting source-swap** — toggling the overlay's 🎤/🔊 source is
  now snappy and no longer breaks the waveform. The swap **aborts** the old
  caption loop instead of waiting out its in-flight transcription (which blocked
  the toggle for seconds on a heavy model), the overlay icon flips
  **optimistically** on click, and — the real bug — the swap now stops *only* the
  caption loop, so the cheap "it hears me" waveform survives (it used to be torn
  down on the first toggle and never came back). The daemon also no-ops a stray
  source-swap in non-toggle states instead of erroring, and a typo'd
  `meeting_preview` mode now fails config validation instead of silently
  degrading to toggle.
- [x] **Minimal recording-indicator overlay** — a second, fully independent
  always-on-top pill for people who want a clear *"you're recording"* cue without
  the live-caption overlay. It shows **only** a pulsing record dot, an audio-reactive
  waveform, and an mm:ss elapsed timer — no transcription text — so it needs no
  live preview at all and works even with preview entirely off. Separate window,
  flag, and saved geometry from the caption overlay; either, both, or neither can
  run. Gated on `interface.recording_indicator` (off by default); enable it under
  **Settings → Live Preview → "Recording indicator"** (`src-tauri/src/indicator.rs`,
  `frontend/indicator.*`).
- [x] **Adaptive whisper-server supervision** — the daemon now spawns *exactly*
  the local whisper-servers the current config needs and no more, from a single
  source of truth (`Config::needed_whisper_servers`): the main server, the live-
  preview server only when preview is on with its own bundled model, and — new —
  an optional **dedicated dictation server** when you opt in. The set reconciles
  live: flip a setting and the matching server spins up or down within a second
  or two, while the servers you didn't touch keep running. A default config still
  runs exactly one server (the main one), so weak boxes are unaffected; power
  users with the headroom can now run all three. Enable the dictation server via
  **Settings → Capture → Dictation → "Dedicated dictation server"** (`[in_place]
  .stt.use_own_bundled_server`); it isolates dictation onto its own process and
  model so a main-server restart or model override can't starve it. Doctor now
  health-checks **every** server it expects to be running, and gained a
  **"dictation is on the slow model"** warning when in-place dictation resolves to
  the heavy main model instead of a fast one.
- [x] **Capture profiles on the Record button** — the Record split-button
  dropdown lists your saved profiles under **Capture profile**; one click swaps
  the whole config for that capture intent (Standup vs Interview, etc.) via the
  existing `switch_profile`. Falls back to **Set up profiles…** → Settings →
  Managers → Profiles when none exist.
- [x] **Dictation voice commands** — in-place dictation now understands spoken
  editing commands: say **"new line"** / **"new paragraph"** to break lines, or
  **"scratch that"** / **"delete that"** to drop the sentence you just dictated.
  Rule-based in `dictation::apply_voice_commands` (segment-anchored, so "a new
  line of code" mid-sentence stays literal) and honored in every cleanup mode —
  fast, off, and llm (the LLM is told to interpret them; the rule pass is the
  fallback). 12 unit tests.
- [x] **Live-preview tuning applies without a restart + clearer help.** The overlay
  now re-reads its feel/perf knobs (reveal speed, waveform, idle window, meeting
  layout) on every recording start, so a Settings change takes effect on the next
  recording instead of only after an app restart (`frontend/overlay.ts`). The
  **Reveal speed** help now spells out that higher = a smoother word-by-word crawl
  and **0 = instant** (not a slower crawl), and that it covers the recording overlay
  (dictation types straight at the cursor). Live Preview and Dictation settings now
  explain the two-server model — a fast preview model on its own server, separate
  from the heavy final one, with dictation borrowing that fast model by default —
  and **Dictation → Custom → main server** warns inline when that main model is large
  (slow dictation), pointing toward Automatic or a cloud provider.
- [x] **System-wide live-preview overlay** — an opt-in, always-on-top, frameless
  desktop window that floats the live caption over any app, even when the main
  window is hidden (`src-tauri/src/overlay.rs`, `frontend/overlay.*`); gated on
  `interface.preview_overlay`. Off by default.
- [x] **Overlay redesign — roomy, clean, properly sized.** The floating caption is
  now a two-zone card: a compact chrome bar (record dot, LIVE/LISTENING, the
  "it hears me" waveform, meeting toggle, close) over a caption area that fills the
  rest of the window, **wraps** long text instead of spilling past the edge, and
  keeps the newest words pinned to the bottom. Sensible default size (540×150) with
  a **minimum** so it can't be shrunk to a useless sliver; resize it taller for more
  lines at once (size remembered across runs). Replaces the old cramped single-row
  card where text was squeezed into a narrow middle column.
- [x] **Dictation rows show the real model + an in-place badge.** In-place recordings
  stored the literal `"in-place"` in the Transcript model column instead of the model
  that produced the text; they now store the actual model (e.g. `ggml-tiny.en`, via a
  shared `WhisperConfig::model_label`) like every other recording, and the detail pane
  shows a small **⌨ in-place** badge (keyed on the persisted `in_place` flag) so
  dictations stay obvious at a glance. Fast-lane dictations (which skip the
  pipeline's LLM auto-title) now also get a cheap, no-LLM **snippet title** from
  the dictated text, so the detail header reads like any other recording — title +
  date + duration — instead of falling back to the bare date as the title.
- [x] **Live-preview wave 1 — smooth, adaptive & it-hears-me.** The biggest live
  preview pass yet, all under the Beta pill until verified:
  - **Adaptive cadence (the record-time crash fix).** When a preview tick takes
    longer than the interval (a heavy model on a modest box), the daemon
    automatically backs the cadence off toward the tick's own cost (clamped to
    8 s) instead of thrashing the machine and wedging the recording. Toggle
    `recording.preview_adaptive` (on by default) to keep a fixed rate instead.
  - **Token-bucket reveal.** The overlay streams words toward the latest text at
    `recording.preview_reveal_words_per_sec` (default 12; `0` = show each update
    instantly) so captions flow like speech instead of jumping a paragraph at a
    time — with an instant correction-snap when whisper revises earlier words.
  - **LIVE ↔ LISTENING state.** The overlay label calms from **LIVE** to
    **LISTENING** after `recording.preview_idle_ms` (default 2500) with no new
    words, instead of showing a frozen caption.
  - **"It hears me" waveform pill.** The overlay shows live audio-level bars
    driven by a cheap daemon RMS loop (a tiny trailing tail at ~15 Hz, no
    transcription, no whisper permit) for single recordings, in-place dictation,
    and meetings — visible proof audio is being captured even between words.
    Toggle `recording.preview_waveform` (on by default).
  - **Heavy-model nudge.** Enabling preview while it shares a heavy local final
    model shows a one-time notice and a one-click **Use a dedicated Tiny model**
    button (Settings → Transcription → Live Preview), steering toward a snappy
    overlay without silently changing your final transcription model.
  - All knobs live in **Settings → Transcription → Live Preview → Feel &
    performance** and are searchable.
- [x] **Word-level timestamps** — transcription providers now capture per-word
  timing (and per-word confidence from Deepgram/AssemblyAI) into a new
  `transcript_words` table, exposed via the `get_words` IPC request / Tauri
  command. The detail pane gains a **🔤 Synced** transcript peek: the machine
  transcript rendered as clickable, time-coded words — click a word to seek the
  waveform, and the word under the playhead highlights as audio plays. The shared
  substrate also unlocks confidence highlighting and tighter diarization
  boundaries.
  - **Fix — the local whisper path stored zero words, so 🔤 Synced was always
    empty.** whisper.cpp's server nests per-word timings inside each segment
    (`segments[].words[]`); the parser only read the OpenAI *cloud* shape (a flat
    top-level `words[]`), so every local-whisper recording persisted no words and
    the Synced view fell back to "no word timings" forever. The parser now reads
    whichever shape the provider returns, and keeps whisper.cpp's per-word
    probability as confidence. (Cloud transcription was already fine.) Existing
    recordings backfill on the next **Transcribe** re-run.
- [x] **Confidence highlighting** — the **🔤 Synced** peek now flags words the
  provider scored below 0.5 with a subtle warning squiggle and a `· N% confidence`
  note in the tooltip, so likely mistranscriptions are easy to spot and check
  against the audio. Words with no reported confidence (whisper-family, most cloud
  STT) are left unmarked rather than mislabelled. Built directly on the word-level
  `confidence` substrate above.
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
- [x] **Coherent diarization turns — no more mid-sentence speaker flips.**
  Per-word attribution scored each word independently off the diarizer's per-frame
  matrix, which over short word windows is dominated by noise — so a single
  continuous voice was chopped into `[Speaker 1] … the fact that women / [Speaker 2]
  going to do what they / [Speaker 1] want …`, flipping speakers mid-phrase (no real
  turn-taking does that). The wall-clock smoothing only caught sub-0.6 s flickers, so
  multi-word noise islands slipped through. Attribution now smooths **word-count
  islands**: a lone word (never a real turn), or a short run (≤ `MAX_ISLAND_WORDS`,
  10 — whisper.cpp emits subword tokens, so a 10-token island is ~5 words)
  bracketed by the SAME speaker on both sides (a noise island inside one voice's
  territory), is absorbed into the surrounding speaker. Per-word attribution is
  kept, so a genuine hand-off *inside* a whisper segment still splits, and real
  sustained turns and true speaker transitions survive — but the mid-sentence chop
  is gone and a solo recording collapses to one speaker (plain prose). Restores the
  coherent turns the segment-level era had, without losing word-level precision.
- [x] **Over-split voices collapsed (voiceprint merge)** — the smoothing above fixes
  *mid-sentence* chop, but it can't fix a wrong speaker *count*: speakrs' VBx stage
  sometimes splits one real voice across two clusters, so a two-person recording
  reported three speakers and the conversation flip-flopped between the phantom pair.
  After the diarizer runs, Phoneme now computes an L2-normalized voiceprint centroid
  per cluster from the per-chunk embeddings and single-linkage-merges any pair whose
  cosine similarity is ≥ 0.50 (calibrated against real recordings: the same voice
  over-splits at ~0.57, genuinely distinct voices sit at 0.33–0.46). The merged
  cluster's per-frame activations are folded into its canonical column and the
  segment spans relabelled, so **both** word-level and segment-level attribution see
  the corrected speaker set. A two-person note now reports two speakers, not three.
- [x] **Orphaned boundary words no longer chop a turn** — whisper transcribes a
  word wherever it hears speech, but the diarizer's segmentation sometimes scores
  no active speaker for that word's exact frames (common at turn boundaries and
  overlaps). Such a word was left unattributed: it rendered with no `[Speaker N]:`
  prefix AND split the surrounding turn into two blocks — the floating-fragment
  "all chopped up" look (`…the company itself is a / cyber / [Speaker 2] weapon?`).
  After smoothing, every unattributed word is now back-filled to a neighbouring
  speaker — a gap inside one turn inherits that speaker, a gap at a hand-off goes
  to the temporally nearest speaker, and leading/trailing gaps attach to the
  first/last speaker — so a turn renders as one clean contiguous block. The
  back-fill only ever copies an existing neighbour, so the speaker count is
  untouched. Local word-level path only for now; the cloud/segment path keeps its
  existing behaviour (tracked separately).
- [x] **Diarized text no longer mangles spacing** — the word-level path rebuilt a
  turn's text by trimming each whisper token and re-joining with a single space,
  but whisper emits *subword* and punctuation tokens whose word boundaries live in
  a leading space it strips. The result was "I don 't know", "over ste pped",
  "ban ning", and a space before every `.`/`,`/`?`. Phoneme now captures whisper's
  leading-space marker per token (`TranscriptWord::leading_space`) and rejoins by
  it, so subword tokens fuse ("over"+"ste"+"pped" → "overstepped") and punctuation
  attaches cleanly ("weapon?", "don't"). Cloud providers, which emit clean words,
  default to normal spacing.
- [x] **Synced (per-word) view honours the same spacing** — the Synced view rebuilds
  its text from the per-word layer, which space-joined every token and so still
  showed "I don 't" / "over ste pped" even after the turn-text fix above. The
  leading-space marker is now **persisted** per word (`transcript_words.leading_space`,
  new migration) and sent over IPC, and the Synced view joins by it — so it reads
  "overstepped" / "don't" / "weapon?" too. **Re-transcribe** to backfill the flag
  on existing recordings (older rows default to space-separated until then).
- [x] **Written words stay atomic across a speaker hand-off** — per-word argmax
  places the speaker boundary on a ~17 ms frame grid, so a token at a hand-off
  could land on the wrong side: a `.` stranded onto the next speaker's turn, or
  `That's` split as `That` [A] / `'s` [B] — the "cut into each other" look. The
  attribution now forces every non-word-start token (punctuation, a clitic like
  `'s`, a subword piece) to inherit the speaker of the word-start it attaches to
  (reusing the same leading-space marker), so a single written word is never
  divided between speakers and a turn never begins with orphaned punctuation. The
  coarser whole-word boundary placement at a hand-off is unchanged (an inherent
  limit of word-level argmax).
- [x] **A monologue's mis-scored island is absorbed, not shown as a phantom turn**
  — the diarizer can mis-score a short stretch *inside* one person's monologue to
  the other speaker (the real case: a 16-token "cyber weapon? I mean, I mean,
  because you don't" stranded between a 31-token question and a 144-token monologue,
  all the same speaker). The island-smoothing now absorbs a same-speaker-bracketed
  run when it is **shorter than both neighbours and under a ~24-token ceiling**, not
  just the old flat 10-token cap — so that phantom collapses into the monologue.
  Genuine turns (the recording's real 100-200-token exchanges) are well above the
  ceiling and never merged.
- [x] **"Treat single recordings as one speaker" option** (`[diarization]
  solo_one_speaker`, off by default). When the local diarizer genuinely hears two
  voices in a one-person recording — a big tonal shift when quoting, or background
  audio — no clustering setting can merge them. This opt-in skips diarization
  entirely for single (non-meeting) recordings, so a solo note is never split into
  `[Speaker N]` turns. Meetings (separate mic/system tracks) and genuinely
  multi-speaker files are unaffected; the mic track's "You" labeling and the
  coherent-turn smoothing above both still apply where relevant.
- [x] **Audio normalization** — optionally boost a quiet recording's gain to a
  consistent peak level before transcribing, so a soft microphone still hands
  Whisper a healthy signal (Settings → Capture → Recording; off by default,
  `recording.normalize` / `recording.normalize_target_dbfs`). Boost-only —
  silent or already-loud recordings are left untouched — and applied to the
  finalized capture (single recordings and each meeting track), never the live
  preview or imported files.
- [x] **Per-app dictation delivery** — set how dictation lands per application:
  **Type**, **Paste**, or **Off** (don't auto-insert; the dictation still saves
  to the library), keyed by the foreground app focused when you stop speaking
  (Settings → Capture → Dictation → Per-app delivery; `in_place.app_overrides`,
  matched case-insensitively by executable stem, e.g. `Code.exe`). Apps you
  don't list use the global **Insert text by** setting, so an empty map behaves
  exactly as before. Foreground detection is Windows-only; elsewhere dictation
  always uses the global mode. A new `phoneme_core::foreground` module reads the
  focused window via Win32 (best-effort: an elevated or unreadable process just
  falls back to the global mode).
- [x] **App-aware AI cleanup (opt-in, off by default)** — when **AI cleanup** is
  the chosen Text polish, optionally add the focused window's title to the
  cleanup prompt so the LLM can adapt to what you're working in
  (`in_place.app_context`). **Privacy-first:** the title can be sensitive, so it
  is never read while this is off; when on it is sent only to your configured
  cleanup provider (prefer a local LLM) and is never logged or stored. An
  `in_place.app_context_denylist` excludes named apps (e.g. a password manager)
  even while it's on.
- [x] **Streaming-type dictation** — type words as they finalize instead of all at
  once on stop. Only clean forward extensions of the streaming preview's committed
  words are typed mid-stream (never a mid-stream backspace, so the cursor doesn't
  churn); on stop a minimal backspace+retype reconciles to the accurate final
  transcript (`dictation::reconcile_edit`). Off by default under
  `[in_place].stream_type`; honored with `type_mode = "type"`.

### Playbook & Custom Hotkeys

- [x] **Deterministic filler removal (no AI)** — a new `filler_removal` Playbook
  kind strips spoken filler ("um", "uh", "er", …) from the transcript in pure
  Rust: no provider, no network, instant and repeatable. It rewrites the running
  text like an LLM Transform — chain it with cleanup either way — and tidies the
  spacing/punctuation the removal leaves behind. Tuned under `[filler]`:
  `words` (conservative default list), `phrases` (multi-word, e.g. "kind of"),
  and an `aggressive` toggle (default off) that gates the meaning-bearing phrases
  so a real "like" / "kind of" is never stripped by accident. Seeded as the
  off-by-default **Remove fillers** entry; the transform lives in
  `phoneme-core::filler`.
- [x] **Re-run through a recipe** — the Re-run / Quick-Model-Switcher modal now
  has a **Recipe to run** picker in Re-run mode: re-run a recording through any
  Playbook recipe (the chain that owns cleanup / title / summary / tags / hooks),
  not just the default. The per-step model tabs still layer one-time overrides on
  top of whichever recipe you pick. Plumbed via a new `recipe_id` on the
  `RetranscribeRecording` IPC, recorded per-job in the recipe ledger and never
  persisted — the same mechanism a custom hotkey's recipe uses.
- [x] **Per-keybind audio source** — a custom hotkey can now pick its capture
  source (microphone or system audio) independently of the global
  `[recording].source`, so you can keep one hotkey for a quick mic note and another
  that records system audio with its own options. Set under a hotkey's **Recipe &
  options**; meeting hotkeys ignore it (a meeting always records both tracks). The
  source actually used is stored on the recording and surfaced in the list's
  **Source** column.
- [x] **The Playbook now owns hooks too — the cutover** — post-transcription
  side-effects (shell commands + webhooks) are **Hook entries** on a recipe, run
  by the recipe executor alongside the LLM steps, not the old top-level `[hook]`
  config. A Hook entry gained a **keyword trigger** (run only when the transcript
  contains a phrase, optional case-matching) and a **"fail the recording"** flag
  (default: failures are surfaced but non-fatal). On first launch a one-time
  `hooks_migrated` migration folds your existing `[hook]` `commands` /
  `keyword_rules` / `webhook_url` into Hook entries on the `default` recipe and
  clears the `[hook]` table — your hooks keep firing, now editable in the Playbook
  and runnable per-hotkey via that recipe. The legacy in-pipeline `[hook]` loops
  still exist but iterate the now-empty table, so a hook fires **exactly once** per
  transcribe (the migration moves it into a recipe Hook step and clears the legacy
  field — never both); `run_on_transcribe` still gates whether a pass fires its
  hooks, and the global `[webhook]` SSRF/HMAC policy still guards outbound POSTs.
  The detail-pane Pipeline popover now shows the real Playbook-hook provenance.
  *(Regression-locked: `configured_hook_fires_exactly_once_per_transcribe`.)*
- [x] **The Playbook is now the source of truth for the LLM-over-transcript
  pipeline** — every recording's cleanup, title, summary, and tag suggestions are
  driven by the built-in Playbook entries and the `default` recipe, not the old
  scattered `[llm_post_process]` / `[title]` / `[summary]` / `[auto_tag]` toggles.
  Edit an entry once in Settings → Playbook and the change flows everywhere it
  runs — the auto-pipeline and the on-demand re-runs (Re-run Cleanup, Re-run
  Summary, Suggest Tags) all read the same entry, so they can never drift apart.
  Behavior is byte-for-byte unchanged for an existing setup; the entries simply
  became the one place the pipeline reads from.
- [x] **One-time config migration into the Playbook** — on first launch after the
  upgrade, your live cleanup / title / summary / auto-tag provider, model, prompt,
  and endpoint are copied into the matching built-in Playbook entries, and the
  `default` recipe is rebuilt from your existing enable flags (a step that was off
  stays off). The reconcile runs once, persists, and sets a `playbook_migrated`
  latch so it never touches your config again; it self-heals on any later config
  reload if that first save ever failed. Your customised prompts and per-step
  providers carry over verbatim — API keys stay where they were, encrypted at rest.
- [x] **New Settings → Playbook section; slimmer Post-Processing & Auto-tag** — a
  dedicated **Playbook** manager lets you edit, add, duplicate, reset, and chain
  the reusable AI "moves" (Transforms, Enrichments, Hooks) and the recipes that
  order them. With the Playbook owning the per-step prompts and connections, the
  Post-Processing and Auto-tag sections are pared back to the few global knobs that
  still belong there, so there's one obvious place to tune each step instead of two.

- [x] **Custom Hotkeys run a Playbook recipe + their own Whisper model** — the
  Settings → Capture → Hotkeys manager (renamed **Custom Hotkeys**) replaces each
  binding's fixed cleanup/title/summary/auto-tag toggles with a **recipe picker**
  (the Playbook chain its recordings run) and a per-hotkey **Whisper model** picker.
  A binding's `recipe_id` (empty = the global `default` recipe, so every existing
  binding is unchanged) is now actually honored end-to-end: the Tauri shell
  registers every enabled custom binding's combo, matches a fired combo back to its
  binding, and sends the binding's recipe + model on the record/toggle request
  (`RecordStart`/`RecordToggle` gained `recipe_id` + `whisper_model`, both
  `#[serde(default)]` so older clients/CLI are unaffected). The daemon stashes them
  in per-recording ledgers (the recipe in a new `pending_recipe`, the model reusing
  the existing `pending_overrides`), and `pipeline::run` resolves THAT recipe and
  applies THAT STT model for just that recording — the same per-job, restore-on-exit
  mechanism a model-override retranscribe uses, so normal recordings and the three
  built-in hotkeys keep the default recipe + configured model with no regression. A
  binding pointing at a deleted recipe falls back to `default` (never a panic, never
  the wrong chain), and the ledger entries are claimed early (before transcription)
  so a failed/canceled recording can't leak a stale entry. (Meeting custom hotkeys
  toggle a meeting like the built-in meeting hotkey; the per-binding recipe/model
  overrides apply to Record / In-place hotkeys.)

### Appearance & themes

- [x] **Full theme palette pass + 5 new themes** — every built-in theme was audited
  token-by-token against its palette's official spec and corrected where it had
  drifted (Tokyo Night's non-canonical cyan, One Dark's amber/orange collision,
  Everforest mixing hard+medium variants, Rosé Pine's two invented border greys and
  an inverted depth order, and every dark theme that was silently inheriting
  Catppuccin's orange for the Queued pill now uses its own palette's orange). Added
  **Catppuccin Frappé** and **Kanagawa** (dark) and **Gruvbox Light**, **Rosé Pine
  Dawn**, and **Tokyo Night Day** (light) — established palettes ported faithfully —
  bringing the picker to 16 themes (11 dark, 5 light), now grouped Dark / Light.
- [x] **Log viewer moved to System → Diagnostics** — the `hook.log` / `daemon.log`
  viewer now lives next to the daemon log level instead of in the Integrations tab;
  Integrations keeps a one-click "View logs in System →" cross-link for hook
  debugging.
- [x] **Detail-pane enrichment polish** — the per-recording **Entities** and
  **Tasks** sections are now collapsible (a chevron header with a per-section
  remembered open/closed state and a count), use kind-coloured chips (people /
  organizations / topics / terms), and sit **below the transcript — between it and
  the notes box** with proper top-and-bottom spacing, instead of being wedged above
  the transcript. The **✂ Clip…** control moved into the main action row beside Play
  / Speed / Re-run / Export / Delete rather than floating on its own line above them.
- [x] **Clip audio is now a waveform modal** — the **✂ Clip…** action opens a
  modal showing the recording's **waveform** with a draggable **start/end region**
  drawn over it: drag the handles, click to seek and ▶ preview, or type exact
  seconds — the handles, the numeric fields, and the live selection readout all
  stay in sync. Export writes the selection to a new WAV beside the recording.
  Replaces the cramped inline clip strip and lays the groundwork for a fuller
  in-app audio-edit surface.
- [x] **"Ask my archive" modal redesigned** — the local-RAG question modal now
  matches the rest of the app: a header with a one-line subtitle, a real question
  field (surface fill + focus ring instead of one that blended into the dialog),
  citations grouped in a titled card, a scroll-styled results region, and inline
  `[n]` citation chips with proper weight. Replaces the unstyled first pass.

### Integration

- [x] **In-app log viewer** — Settings → Destination & Integrations now has
  **View hook log** / **View daemon log**: a read-only modal that tails the last
  ~400 lines so a hook that silently does nothing is debuggable without leaving
  the app. Backed by a `tail_log` Tauri command with an allowlisted set of log
  basenames (no path traversal) that resolves the daily-rolled `daemon.log.*`
  automatically. `LogViewer.ts`.
- [x] **MCP server (`phoneme-mcp`)** — a thin Model Context Protocol bridge
  (JSON-RPC 2.0 over stdio) that maps tools straight onto the daemon's existing
  IPC requests with near-zero business logic. Observe-only (never spawns a
  daemon); a down/erroring daemon and any bad input surface as clean MCP tool
  errors, and the stdio framing is bounded (8 MiB, mirroring the IPC codec)
  against oversized or unterminated frames. Drop-in for Claude Desktop and other
  MCP hosts — see [MCP Server](docs/developer-guide/mcp_server.md).
- [x] **Agent toolset grows from read-only to "act on it"** — the `phoneme-mcp`
  bridge and the in-tree agent registry (`crates/phoneme-agent-core`) now expose
  fourteen tools, kept in lockstep (same names, same IPC requests, opposite
  direction). Beyond the original read-only five (`start_recording`,
  `stop_recording`, `get_transcript`, `search_recordings`, `list_recent`) an
  agent can now act on recordings: `set_title`, `set_favorite`, `suggest_tags`,
  `list_tags`, `summarize`, `rerun_cleanup`, `retranscribe` (heavy — re-runs the
  whole pipeline, optional one-time model override), `more_like_this`, and
  `get_words` (word-level timings, e.g. for caption/SRT export). Each stays a
  pure args → `Request` mapping; the mutating ones answer with a short
  confirmation and never persist their per-run model overrides to config.
- [x] **Local REST API (`phoneme-rest`, off by default)** — a localhost `axum`
  bridge over the daemon, bound to `127.0.0.1` only (the loopback trust
  boundary). Each endpoint maps one HTTP call to one `phoneme-ipc` `Request`
  (recordings list/get/segments, search, record start/stop, status, health), and
  `GET /api/events` streams `DaemonEvent`s as Server-Sent Events. Never
  auto-spawns the daemon (down → 503, bad input → 400), and the daemon
  round-trip is bounded so a wedged daemon can't hang a request. Opt-in via
  `[rest_api] enabled = true` (`port` default 3737). See
  [REST API](docs/developer-guide/rest_api.md).
- [x] **REST API breadth** — the bridge maps a much wider, high-value slice of
  the IPC surface beyond the original list/get/segments/search/record set:
  read routes for words (`GET /api/recordings/{id}/words`), more-like-this
  (`/similar`), a recording's tags (`/tags`), the tag list (`GET /api/tags`),
  and the pipeline queue (`GET /api/queue`); mutating routes to set/clear a
  title (`POST /api/recordings/{id}/title`), star (`/favorite`), attach
  (`POST .../tags`) and detach (`DELETE .../tags/{tag_id}`) a tag, re-run
  cleanup/summary (`/cleanup`, `/summary`), and start/stop a meeting
  (`POST /api/meeting/{start,stop}`). Each is the same thin one-HTTP-call →
  one-`Request` mapping, reusing the existing id-validation (`400`), error→status,
  and loopback/`Origin` (CSRF) guards. Unit + route tests cover every addition.
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

- [x] **CI inject-guard gate** — `cargo test` in CI now runs with
  `PHONEME_DISABLE_INPUT_INJECTION=1`, and the daemon E2E harness spawns its real
  daemon with the same var set, so tests can never type/paste into a real window
  (the daemon binary isn't `cfg!(test)`, so the env var is its only guard). A CI
  step also fails if any of the `type`/`paste`/`reconcile` blocking paths stops
  checking `input_injection_disabled()`, so the guard can't regress.

- [x] **Browser preview without the daemon** — a dev-only mock IPC
  (`frontend/src/services/tauriDevMock.ts`) feeds canned, fully-synthetic data so
  the whole UI renders in a plain browser (`cd frontend; npm run dev`), for fast
  layout / keyboard-nav / animation work and screenshots without launching the
  native window. Installs only in a Vite dev build with no real Tauri runtime, and
  is dead-code-eliminated from production builds. See the Frontend Developer Guide
  §4.3.

- [x] `phoneme completions <bash|zsh|fish|powershell|elvish>` prints a shell-completion script to stdout (pure local, no daemon needed).

- [x] A fully-commented `config.example.toml` and `.env.example` at the repo
  root document every config key (with defaults) and every runtime env var.

- [x] **One tool catalog for the agent + MCP** — `phoneme-mcp` no longer keeps a
  second, hand-maintained copy of the tool list. It now depends on
  `crates/phoneme-agent-core` and builds its MCP `tools/list` and `tools/call`
  dispatch *from* that registry (`tools.rs` is a thin adapter that re-shapes the
  registry into the MCP wire format and renders results). The registry is the
  single source of truth for names, schemas, and the arg→`Request` mapping, and a
  test asserts `phoneme-mcp`'s exposed names equal the registry's so the two can
  never drift again.
- [x] **MCP tool breadth — meetings + speakers** — the shared `phoneme-agent-core`
  registry (and so the `phoneme-mcp` surface) grew from sixteen to **thirty-one**
  tools, all mapping to existing IPC requests. New: `get_segments` and
  `approve_tag_suggestion` / `dismiss_tag_suggestion`; meetings (`start_meeting`,
  `stop_meeting`, `list_meeting`); speaker correction + recognition
  (`set_speaker_name`, `reassign_speaker_segment`, `merge_speakers`,
  `split_speaker`, `recognize_speakers`); and the named-voice library
  (`list_named_voices`, `rename_named_voice`, `merge_named_voices`,
  `forget_named_voice`). Each is a pure args → `Request` mapping with bounds-checked
  speaker labels / segment indices, and the drift-guard test (MCP tools == registry)
  derives its counts from the registry so it can't desync. See
  [MCP Server](docs/developer-guide/mcp_server.md).

- [x] **`recorder.rs` split into a directory module** — the 3.3k-line daemon
  recorder is now `recorder/{mod,preview,meeting}.rs`: `preview.rs` owns the live
  caption/waveform loops and their pure stitching helpers, `meeting.rs` owns Meeting
  Mode, and `mod.rs` keeps the single-recording lifecycle and shared
  `DaemonRecorder` state. Pure refactor — `DaemonRecorder`'s public API is byte-
  identical and the daemon test suite is unchanged.

### Performance

- [x] Semantic search holds the deserialized embedding corpus in memory, so
  repeated queries (and the upcoming RAG retrieval) skip re-reading and
  re-decoding every vector BLOB from SQLite; invalidated on any re-embed or
  delete, bounded for large libraries.
- [x] **Optional ANN vector index for semantic search** — an opt-in
  [usearch](https://github.com/unum-cloud/usearch) HNSW index that replaces the
  brute-force cosine scan with sub-linear nearest-neighbour search on large
  libraries. Gated **twice** and **off by default**: the cargo feature
  `ann-usearch` (not in any default build — a stock binary is byte-for-byte
  today's, zero new native code) *and* the runtime flag `semantic_search.ann.enabled`
  (default `false`). When on, the index only narrows *which* candidates are
  scored; the existing exact cosine re-score, meeting-dedupe, and RRF fusion are
  unchanged, so displayed scores match brute force and the index can at worst
  miss a tail result (tunable via `oversample` / `expansion_search`). The
  brute-force scan stays the guaranteed fallback: a missing/stale sidecar,
  dimension mismatch, count drift, or any query error logs a `warn` and falls
  through to it — search never errors. The on-disk index is a disposable sidecar
  (`catalog.ann`) rebuilt from SQLite on any integrity doubt; the daemon
  background-builds it after the embedding backfill drains (startup never blocks),
  and incremental insert/delete/re-embed keep it in step through the existing
  embedding-cache choke points. The Doctor reports its health
  (disabled / rebuilding / healthy / stale). Build the feature lane with
  `cargo build --features ann-usearch`.

### Reliability & polish

- [x] **Extract tells you when no AI provider is configured** — clicking **Extract**
  for entities, tasks, or chapters with no usable LLM provider (every step set to
  `provider = "none"`, or a local model that isn't reachable) now returns a clear
  error — "set one under Settings → Post-Processing" — instead of silently doing
  nothing and looking broken. The automatic pipeline still skips the step quietly,
  so a missing model never fails a recording; only the explicit on-demand button
  surfaces it.
- [x] **Transcript editor scrolling & focus** — the mouse wheel over the
  transcript editor now scrolls the detail pane when the editor itself has nothing
  more to scroll (CodeMirror used to trap the wheel and freeze the page), and
  keyboard-focusing the editor no longer yanks the transcript to the middle of the
  pane — the focus you rely on stays, the jarring re-center is gone.
- [x] **Transcript / notes editors render reliably** — pinned a single
  `@codemirror/state` instance (Vite `resolve.dedupe` + `optimizeDeps`) so the
  editors never hit "Unrecognized extension value" and fail to mount.
- [x] **App-health pill** moved to the far right of the header bar and mirrored
  into the Settings page (shared `state/health.ts` store → one Doctor poll feeds
  the header pill, the Settings pill, and the failure banner); both pills are
  pixel-identical and the dot no longer resizes as health resolves.
- [x] **Settings ⚙ split button** is byte-identical between the header and the
  Settings page at any UI font size / display scaling (height, caret box, and
  divider matched; whole-pixel anchor); inside Settings it reads **← Go Back** at
  the same size. The Settings panel content now starts below the floating button.
- [x] **Keyboard glow consistency** — in the detail-pane dropdowns (Views /
  Versions / Pipeline / Speed / Export) the cursor glow now stays on the trigger
  button and the option shows its own border (matching the header dropdowns),
  instead of following into the popup and stranding on Escape. And the glow now
  tracks a click into an editor while it's hidden, so exiting (Shift+Esc) resumes
  from where you clicked rather than gliding in from a stale spot.
- [x] **Settings consistency** — Live Preview field hints moved to the shared
  value-column help style; the bundled-model list aligns with the other inputs.
- [x] **Library count badges** — the sidebar's Library rows (All Recordings /
  Voice Notes / Meetings / In-Place / Favorites) now carry the same right-anchored
  count badge as the tag rows, fed by a single `kind_counts` IPC (one SQL pass;
  `Catalog::kind_counts`) and refreshed off recording lifecycle + favorite events.
- [x] **In-Place Library filter** — a new **In-Place** row (above Favorites) filters
  to recordings captured via in-place dictation, applied in SQL before pagination
  (`ListFilter.in_place`) so pages stay full.
- [x] **Status filter dropdown** matches its pill width — the popup is pinned to the
  button via the customizable-`<select>` model (`appearance: base-select` +
  `anchor-size`), instead of the native popup spilling wider; degrades to the
  classic select on older runtimes.
- [x] **Escape closes every dropdown** — the Speed / Export action-row menus and the
  Saved-searches dropdown now close on Escape (they only closed on outside-click
  before), matching the Views / Versions / Pipeline menus and modals. Escape never
  bubbles up to close the open recording.

- [x] **Safe "Re-import recordings from disk"** — a NON-destructive recovery path
  (`ReimportFromDisk` IPC, `phoneme doctor --reimport`): scans the audio directory
  and re-links any `.wav` whose RecordingId has no catalog row — re-creating the
  row from the file (original id + timestamp preserved, **no copy**) at `queued`
  and re-running the pipeline. Never deletes or touches existing rows; files whose
  names aren't valid ids are skipped. This is the safe counterpart to the
  DESTRUCTIVE `doctor --rebuild-catalog`, whose help text now states plainly that
  it deletes the catalog (transcripts/tags/notes/titles are DB-only and lost; the
  daemon does **not** reconstruct rows from audio) and points at `--reimport` for
  recovery. Also surfaced as a **Doctor button** ("↻ Re-import from disk"): one
  click dry-runs and reports how many orphaned files it found, a second confirms
  and re-links them.
- [x] **Diarization tuning knobs** — Settings → Diarization (local) now exposes the
  speakrs pipeline knobs: **merge gap** (seconds; how aggressively same-speaker
  turns coalesce), **speaker keep threshold** (drop weak clusters), and
  **turn reconstruction** (smoothed vs standard, with a smoothing-strength ε).
  They map onto speakrs' `PipelineConfig` at load time, and the diarizer cache is
  keyed on the whole `[diarization]` config so changing any knob reloads the
  pipeline with the new values. Defaults match today's implicit behavior
  (0.25 / 1e-7 / smoothed 0.1), so existing configs are unaffected.
- [x] **Delete no longer silently keeps audio forever** — the delete dialog used
  to remember a "keep the audio file" choice alongside "Don't ask again", so one
  past keep-audio delete quietly turned *every* later delete into keep-audio: rows
  vanished but `.wav` files piled up as orphans the user thought were gone. Now
  "Don't ask again" only ever pins the safe full delete; keep-audio is always a
  deliberate, per-delete choice, and any stale remembered keep-audio mode is
  cleared on the next delete. (The daemon's delete was always correct — this was
  the UI footgun.)
- [x] **Doctor flags orphaned audio** — a new **"Orphaned audio"** check counts
  `.wav` files on disk with no library entry (what accumulates from keep-audio
  deletes, and what "Re-import from disk" would resurrect), so it can't grow
  silently. Surfaced identically in the CLI (`phoneme doctor`), the Doctor view,
  and the Doctor modal via a shared builder.
- [x] **UI font size is a real font size now** — the Appearance → font-size setting
  drives the root `font-size` (`--ui-font-size`), and every text size across the app
  is expressed in `rem`, so changing it scales the interface text up/down cleanly.
  It replaces an earlier `zoom`-based approach that magnified spacing/boxes and could
  push the layout off-window with no way to scroll back. At the 14px baseline nothing
  changes; other sizes scale text without breaking the fixed-viewport layout.
- [x] **Keyboard navigation inside modals & popups** — with vim or arrow nav on, every
  modal (the Re-run / Models picker, Doctor, confirmations, Tag Manager, the log viewer,
  …) is now keyboard-drivable the same way as the rest of the app: `h`/`l`/`j`/`k` + arrows
  rove a `.kbd-cursor` highlight over the dialog's controls, `Enter` activates (buttons fire,
  fields open for typing), `Esc` closes. One generic driver in the keyboard layer covers
  every current and future `.modal-overlay`, so there's no per-modal wiring; the confirm
  dialog keeps its own capture-phase `Enter`/`Esc`, and a destructive delete starts the cursor
  on Cancel. Also broadened the modal guard to the `*-modal-overlay` variants (compare /
  speakers), fixing a latent bug where their keys leaked to the detail pane behind them.
- [x] **Escape leaves the Settings panel** — a bare `Esc` closes Settings (with the
  unsaved-changes guard); the search box, open dropdowns, and layered modals still consume
  their own `Esc` first. (Full keyboard nav *inside* Settings is intentionally not wired yet.)
- [x] **Sidebar highlight no longer lingers in the header** — moving from a sidebar filter
  to the top bar (by keyboard or click) now clears the sidebar cursor so only the header is
  highlighted, while the sidebar still remembers where you were when you return.
- [x] **Tag-chip editor is keyboard-driven** — the inline rename/recolor popover now roves
  with the `.kbd-cursor` box (`h`/`l`/`j`/`k` + arrows across color · name · Save · Remove ·
  Cancel), `Enter` activates, `Esc` steps back — instead of relying on native focus.
- [x] **`dd` deletes the whole selection** — with multiple recordings selected, the vim
  `dd` motion now deletes every selected one (matching the `Delete` key and the bulk bar)
  instead of only the row under the cursor. Also fixed a flicker when an undoable delete's
  grace period lapsed: the rows briefly flashed back onto the list before vanishing, because
  the hide set was cleared before the list re-fetched — the refresh now lands first.
- [x] **Animated keyboard cursor** (`interface.cursor_animation`, opt-in) — the roving
  `.kbd-cursor` highlight can now glide between controls, with a translucent accent glow that
  chases it and an optional fading streak — inspired by smear-cursor.nvim / mini.animate.
  Four modes in **Settings → Appearance → Keyboard cursor animation**: `off` (default),
  `glide`, `smear` (glide + a streak on bigger jumps), `trail` (a streak on every move). Purely
  additive (the real outline still marks position), honors `prefers-reduced-motion`, and is a
  single compositor-light overlay so it stays cheap on modest machines.
  friendly counterpart to vim navigation: `←`/`→` move between the sidebar, list, and
  detail panes; `↑`/`↓` move within the list, sidebar filters, and detail rows; `Enter`
  opens/activates; `Esc` steps out. It drives the **same** pane/grid cursor engine as
  vim nav, so the two can run together (arrows _and_ `h`/`l`/`j`/`k`). Toggle it in the
  first-run wizard or **Settings → Appearance → Arrow-key navigation**; bare `h`/`j`/`k`/`l`
  and the vim-only extras (`dd`, `zz`, `gg`/`G`, `x b`/`x /`, ±5s scrub) stay behind
  `vim_nav`. Default off, so an upgrade never changes what the arrow keys do.
- [x] **Cleaner keymap tiers + `g`-chord consistency** — the keyboard layer now has a
  documented three-tier model (NORMAL always-on · VIM `interface.vim_nav` · EDITOR
  `editor.vim_mode`). The "go to a place" `g`-chords `g b` (sidebar) and `g 1`/`g 2`
  (split panes) are no longer gated behind vim nav — they join their already-default
  siblings (`g d`/`g l`/…), so every `g`-destination works for everyone. `Tab` /
  `Shift+Tab` (the normal way to move within a region) and the promoted chords are now
  listed in the `?` cheat-sheet.
- [x] **Detail-pane & keyboard-nav polish** — the **Pipeline** provenance popover
  now floats free of the detail pane's scroll box (fixed positioning, clamped into
  the viewport), so long model names like `ggml-large-v3-turbo` wrap cleanly inside
  it instead of crowding or being clipped at the pane edge. Keyboard nav gained
  parity across panes: **focus follows your mouse in the header strip** (clicking a
  header control places the roving cursor on it, like the list/detail/sidebar
  already do), **k at the top of the sidebar** drops up into the search bar (matching
  k at the top of the list), and **Shift+Esc in the Notes editor** leaves the editor
  back to the detail pane (matching the transcript editor). Also: the Settings panel
  reliably fills its area again — the custom-element host no longer collapses the
  height chain (`display: contents`), so short tabs keep the Save/Close footer pinned
  to the bottom instead of floating mid-window (or, in the regression, blanking out).
- [x] **Split-view keyboard nav** — with two recording panes open (`\`), `h`/`l`
  now cross between them at a row's edge (the detail grid's per-row `h`/`l` still
  walks a row's buttons in the middle), and `g 1` / `g 2` jump straight to the
  left / right pane.
- [x] **`x b` / `x /` toggle the sidebar / top bar** — vim-mode twins of the
  `Ctrl+B` / `Ctrl+/` chrome toggles, on a new `x` prefix leader. (`x b` falls
  back to the list if it hides the sidebar while your cursor was in it.)
- [x] **Pane cursor memory everywhere** — every pane now remembers where your
  cursor was and restores it on return, not just the detail pane: leave the
  **sidebar** or the **header strip** (e.g. down to the list, back up with `k`)
  and you land on the same cell/control you left, instead of snapping to the top
  row / search box each time. First entry still lands on a sensible default; the
  header falls back to the search box and `g /` always goes there.
- [x] **Vim nav won't passively enter a hidden bar** — with the top bar hidden
  (Ctrl+/, zen, focus mode), `k` at the top of the list/sidebar/detail no longer
  lands an invisible cursor on it (you'd get stranded) — it stays put. The
  deliberate jumps still work and force the bar open: `g /` reveals + roams the
  top bar, `g b` reveals + enters the sidebar; `/` peeks the bar to search.
  (`h`/`l` already skip hidden panes.)
- [x] **`:w` / `:wq` save again from the vim Ex dialog** — the `:` command runs
  as a CodeMirror panel that holds focus while the command fires, so the editors'
  content-only `hasFocus` check skipped the save. They now accept focus anywhere
  in the editor (the content **or** its `:` dialog), so `:w` / `:wq` / `:x`
  commit the transcript and notes again.
- [x] **Edited transcripts re-sync the Synced & Timeline views** — when you edit
  and save a transcript, the per-word and per-segment timing layers are re-flowed
  onto your new text (`phoneme_core::realign`), so the **Synced** (per-word) and
  **Timeline** views and click-to-seek follow the edit instead of drifting:
  unchanged words keep their exact timing, inserted words are interpolated into the
  gap between the surrounding anchors, deleted words drop out. A monotonic word diff
  means the timeline never runs backwards even after a reorder, and speaker
  attribution is preserved (the `[Speaker N]` marker when numeric, else the matched
  word's index — so renamed speakers keep mapping). **No model run** — it reuses the
  audio's known timings, so it's instant and offline. On by default; **Settings →
  Editor → "Re-sync … views when you edit"** turns it off to keep the original
  machine timings. (True forced re-alignment of edited words against the audio is a
  roadmap item — it needs an aligner model.)
- [x] **Security pass + audit follow-ups** — a deep security audit; the one real
  finding fixed: `SemanticSearch`/`MoreLikeThis` now clamp the client-supplied
  result `limit` (≤1000) so a huge value can't force an unbounded allocation over
  the IPC pipe. (Rejected as not-applicable after review: "SSRF" on the
  transcription/LLM URLs — those are user-set and legitimately point at localhost
  Ollama/whisper, so blocking private ranges would break local-first; `export
  --out` "traversal" is just the user's own CLI output path; `UpdateTranscript`
  is already bounded by the 8 MiB IPC frame cap.) Plus: a transiently-failed
  recording now shows **Queued** while it waits to retry (not a frozen
  "Transcribing"), and a re-run that overrides the cleanup provider/endpoint logs
  an audit line (never the key).
- [x] **Synced view spacing — the last mile** — the per-word `leading_space` marker
  was persisted and serialized, but the `GetWords` IPC built a hand-rolled JSON
  object that *omitted* it, so the Synced view never received it and still
  space-joined every token ("I don 't know"). `GetWords` now includes
  `leading_space`, so the Synced view renders the same clean spacing as the
  transcript ("I don't know", "overstepped", "weapon?").
- [x] **Queue requeue can't silently stall** — if requeuing a transiently-failed
  recording itself fails, the worker now marks it failed (surfaced in the UI)
  instead of leaving it stuck in `processing/` until the next daemon restart.
  The hot-path `pending_overrides` mutex also recovers from poisoning instead of
  panicking the daemon.
- [x] **Dismiss one failed item** — the failed-recordings panel now has a per-item
  **Dismiss** (clears that recording's `failed/` quarantine marker and hides the
  row; the recording stays in the library), the counterpart to **Clear failed**.
  New `DismissFailed` IPC + `phoneme queue dismiss-failed <id>`.
- [x] **Audit hardening (verified findings)** — a whole-app audit pass; the
  confirmed-real items fixed: a Deepgram speaker turn now advances its segment end
  time even when a word lacks an `end` timestamp (falls back to the word's start,
  matching the segment-creation site) so later seeks don't mis-align; webhook
  `custom_headers` are redacted to **names only** in `Debug` so a header secret
  (e.g. an `Authorization` token) can't leak into logs; the tray menu record/stop
  listeners catch `invoke` failures instead of raising an unhandled rejection.
  (Several flashy "findings" were false positives — no `start`/`start_meeting`
  deadlock, no `tag_id` SQL injection (it's an `i64`), and `showOverlay()` already
  self-catches — left unchanged with a clarifying comment where useful.)
- [x] **Docs caught up to the v1.8 work** — the user + developer guides now cover
  the diarization quality passes (word-level attribution, coherent-turn smoothing,
  voiceprint merge, orphan back-fill, atomic words/spacing) and `solo_one_speaker`,
  the full recording-status set (incl. Queued, Paused, and the optional-step
  failures vs terminal failures), named speakers, the Ollama streaming idle-timeout
  + small-model-for-low-RAM guidance, that live preview is skipped during in-place
  dictation, the `phoneme cleanup --api-url/--api-key` overrides, and a Claude Code
  `.mcp.json` MCP setup section.
- [x] **Live preview no longer breaks in-place dictation** — an in-place (quick)
  dictation started the streaming-preview loop too (it was gated only on
  `streaming_preview`, not on the dictation flag). The loop's per-second
  transcription ticks then contended with the dictation's own latency-critical
  transcribe-and-paste on the single serial whisper permit — and `stop()` waited
  out an in-flight preview tick before the dictation could transcribe — so the
  paste was delayed or never happened ("it's constantly listening, it never pastes
  the quick transcription"). In-place dictation now skips live preview entirely: a
  quick dictation has no preview overlay to feed, so it goes straight to
  transcribe → paste with the whisper permit free.
- [x] **Local LLM post-processing no longer times out mid-generation** — the
  cleanup/summary/title steps applied the `[llm_post_process].timeout_secs`
  (default **30 s**) as a *total* deadline on the request, including a **streaming**
  Ollama response. A healthy but slow local generation on a CPU box (or a cold
  model load under memory pressure) blew past 30 s and was aborted mid-stream,
  surfacing as the opaque `Ollama stream error: error decoding response body` and
  a `cleanup_failed` / `summary_failed` recording. The streaming path now bounds
  the **idle** time between chunks (floored to ≥120 s, also covering the first-token
  cold load) and lets total generation run as long as tokens keep arriving; the
  non-streaming Ollama call gets the same ≥120 s floor. A genuine stall now fails
  with an actionable message ("the model may be loading/swapping under memory
  pressure — try a smaller model or raise `timeout_secs`") instead of a decode
  error.
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
- [x] **Live-preview overlay no longer freezes the app when dragged.** The
  caption card was a `data-tauri-drag-region`, so moving it entered Windows'
  modal move-loop — which, on this transparent always-on-top WebView2 window,
  blocked the shared event loop and hung the whole app (often permanently). It
  now drags via manual `setPosition` (coalesced to one move per frame), which
  never enters the move-loop.
- [x] **Library backup + dependency-detection no longer block the UI thread.**
  `export_library_zip` (zip create + per-WAV read/deflate) and `wizard_detect_deps`
  (spawns the `ollama` CLI + stats the filesystem) were `async` Tauri commands
  doing heavy synchronous work on an async worker; they now run inside
  `tokio::task::spawn_blocking`. (The two confirmed fix-now items from the
  external-audit validation pass — the rest were false, already-optimized,
  intentional, or large refactors deferred to the roadmap.)
- [x] **Title column in the recordings list.** The display title (previously only
  bolded inside the transcript-snippet cell) is now a standalone, toggleable,
  reorderable column in **Settings → Appearance** — width, header, and a cell
  that shows the title (em-dash + muted for untitled rows). Off by default (the
  snippet already shows it). The per-step model columns it sits beside
  (Transcription / Post-Processing / Summary Model) were already present.
- [x] **Searching a tag returns its recordings in semantic mode too.** Plain
  (FTS) search already matched tag names; the semantic/hybrid path fused only
  vector + transcript-FTS, so a tag query missed tagged recordings there. A new
  `tag_ranking` folds tag-name matches into the hybrid search's exact-intent
  (lexical) set, so it works in both modes.
- [x] **Favorites star is a ⭐ emoji** in the list column (and its header) instead
  of the flat `★`/`☆` glyphs — bright when starred, a faded ghost (dimmed +
  grayscaled) when not, brightening on hover.
- [x] **No more double toasts on summary / tag re-runs.** A standalone ✨ Summary
  or suggest-tags run emits a pipeline-stage event (for the queue's active-item
  display) AND the step's dedicated `summary_updated` / `tag_suggestions_updated`
  event; notifications toasted on both. The `summarizing`/`tagging` stages now
  stay quiet (still tracked, so a later "done" still reads "Summarized ✓ —
  recording ready") and their dedicated event owns the single toast.
- [x] **Optional-step failures are now filterable, searchable statuses.**
  Previously only transcription and hooks had failure visibility; cleanup,
  auto-title, auto-summary, and auto-tag failed silently (log-only), then briefly
  toast-only. They now become real terminal statuses — `cleanup_failed`,
  `summarize_failed`, `title_failed`, `tag_failed` — exactly like `hook_failed`:
  the transcript is intact and the recording is fully usable, only that one
  enrichment didn't land. The earliest failed step wins the status, and its
  reason is persisted on the row (`error_kind` + `error_message`) so the failure
  survives a restart and the **why** shows in the failed panel and `phoneme list`.
  Filter for them in the status dropdown or `phoneme list --status tag_failed`;
  the failed panel lists them with a per-step "Cleanup / Summary / Title /
  Tagging" label and a Retry. The daemon still emits the matching `CleanupFailed`
  / `TitleFailed` / `TagFailed` / `SummaryFailed` events for live toasts, and a
  user-skipped stage still reads as "skipped", never a failure.
- [x] **A recording WAITING in the queue now reads "Queued", not "Transcribing".**
  Enqueue (a finished recording, a meeting track, a re-transcribe, an import) sets
  the new non-terminal `queued` status; the pipeline worker flips it to
  `transcribing` only when it actually claims the item. So a backlog of three
  recordings shows one "Transcribing" and two "Queued" instead of three lying
  "Transcribing". `queued` is filterable everywhere statuses are (status dropdown,
  `phoneme list --status queued`), and crash-recovery sweeps an orphaned `queued`
  row (no inbox file) to `transcribe_failed` like the other in-flight states. The
  dictation fast lane is unaffected — it transcribes immediately, so it stays
  "Transcribing".

### GUI parity

- [x] **Export an audio clip from the app.** The detail pane gained a **✂ Clip…**
  control under the waveform: pick a **Start** and **End** in seconds (or hit
  **⟱ Playhead** to set either from the current playback position) and **Export
  clip** writes that range to a new WAV — the GUI front for the existing
  `phoneme clip <ID> <START> <END>` (same `ExportClip` request and sibling
  `_clip_<start>-<end>.wav` default path). The range is validated client-side
  first (end > start, within duration, end clamped to the recording's length like
  the CLI), so an empty or back-to-front range shows a hint and never sends; the
  saved path comes back in a success toast.
- [x] **AI-activity log persists across restarts.** The 🧠 "AI Activity" popout
  was in-memory only — every completed cleanup/summary prompt+response vanished
  when the app reopened. The daemon now writes each finished streaming LLM
  session (everything through `run_llm_stage`, incl. re-runs) to a durable
  `ai_activity` table; the popout loads recent history on open (`list_ai_activity`
  IPC) and the live stream appends to it. The table is pruned to a bounded recent
  window (newest 1,000) so it never grows without limit, and `recording_id` is
  kept unlinked so deleting a recording doesn't erase the audit trail.
- [x] **Settings reorganized into nine focused tabs.** The old six-tab grouping
  (where **System** alone held five sections) is split so each concern is its
  own tab: **Transcription · Live Preview · Diarization · Capture ·
  Post-Processing · Appearance · Recall · Managers · System**. A single
  data-driven section registry is now the source of truth — it feeds the tab
  rail, the per-tab mount order, the all-sections search index, and the ⚙
  jump-to-section menu, so the three could never drift out of sync again. Search,
  per-field keyword matching, result breadcrumbs, and the Managers sub-tabs are
  unchanged; legacy deep-links (`tags`, `managers/profiles`, `postprocessing`, …)
  still resolve.
- [x] **Choose the interface font & size.** Settings → Interface gained an
  **Interface font** picker (Inter default, plus Windows-bundled and common
  cross-platform families incl. monospace options) and an **Interface font
  size** (12–18px). Both drive app-wide CSS vars (`--ui-font` / `--ui-font-size`
  off `interface.ui_font` / `ui_font_size`); a chosen family is prepended to the
  bundled stack so an uninstalled font falls back cleanly, and transcript/code
  blocks keep their fixed monospace.
- [x] **One Export ▾ menu per recording** — the separate Export and 💬 Captions
  buttons are now a single dropdown: **Transcript** (.txt), **Captions** (SubRip
  .srt / WebVTT .vtt, matching `phoneme export --captions`), and **All data**
  (.json — the catalog row plus machine segments). Every export now writes
  **server-side** via the bridge process, fixing "Caption export failed:
  fs.write_text_file not allowed" — the WebView no longer needs the fs plugin's
  write permission for an arbitrary save-dialog path. The merged-meeting Export
  was on the same broken path and is fixed too.
- [x] **Detail-pane action row tidied.** ✨ Similar moved up into the recording's
  title bar (beside fullscreen/close); **Copy** is a 📋 button that lives in the
  transcript editor's header button row — a sibling of the ✓ Edited badge and
  Save Changes, not an overlay — so it never collides with them; it shows only
  when the transcript is saved (hidden while editing) and flashes a ✓ on copy.
  The **notes box** gained the same header Copy button (same show-when-clean
  rule). 🗑 **Delete**
  sits last in the action row (Play · Re-run… · Export ▾ · Delete), styled as the
  destructive action. The header meta line is reordered to **status → date/time →
  duration → source**, and the source is now just its 🎤/🔊 icon (full name on
  hover). The Reveal button is gone — the file path in the footer is now clickable
  to reveal it in the OS explorer. The footer's model line became real **pipeline
  provenance**: every stage that actually touched this file, in the order the
  daemon ran them — capture → transcription (+ diarization) → cleanup →
  auto-title → hook → summary → auto-tags — naming each step's model where it's
  recorded per-recording and omitting steps that didn't run. The daemon now also
  records the **auto-title** and **auto-tag** LLM models and a **cloud diarizer's**
  model per recording (new `title_model` / `tag_model` / `diarization_model`
  columns), so those steps name their model too once a recording is (re)processed;
  the local speakrs diarizer has no model name, so it still reads "diarized".
- [x] **More recordings-list columns + stickier widths.** The list gained
  toggleable, reorderable **Title Model**, **Auto-Tag Model**, and **Diarization
  Model** columns (alongside the existing per-step model columns), the **Source**
  column shrank to just its 🎤/🔊 icon, and column widths now persist **by column
  name** (per device) — so adding, removing, or reordering a column no longer
  resets every width.
- [x] **Auto-tag suggestion chips tidied.** Dropped the redundant ✨ from each
  suggested-tag pill (the row already reads as suggestions); the bulk buttons read
  **✓ All** / **✕ Clear**; and the tag input no longer eats `j`/`k` (an old
  empty-box vim-browse shortcut swallowed the first letter of tags like
  "javascript" — gone; use ↑/↓ to browse suggestions).
- [x] **Title model in the quick model switcher.** The Re-run / Models modal
  gained a **Title** tab alongside Summary and Auto-tag (a blank provider inherits
  the post-processing connection). "Save as default" writes `[title]`; **Run
  once** carries a one-time title-model override (new `title_model` field on the
  re-run-all IPC) that enables the LLM title step for that run.
- [x] **Timeline peek reads as turns, not whisper fragments.** The 🕒 Timeline
  list rendered one row per raw whisper segment, which breaks mid-sentence and
  emits tiny fragments — illogical splits. It now merges consecutive same-speaker
  segments into coherent rows (breaking on a sentence end, a >2s gap, a speaker
  change, or a length cap). Click-to-seek and the playhead-follow highlight still
  land on real audio, and the dual-timeline meeting sync is unchanged.
- [x] **Webhook safety toggles** — Settings now exposes
  `webhook.allow_private_network` and `webhook.allow_http` (previously
  TOML-only) with plain-language warnings.
- [x] **Every new backend surface is configurable in the GUI** — the webhook
  **signing secret** (`webhook.hmac_secret`, a masked/DPAPI password field) and
  **custom headers** (`webhook.custom_headers`, an add/remove row editor) join
  the safety toggles in Post-Processing → Destination & Integrations; and a new
  **System → Integrations** section turns on the **REST API bridge**
  (`rest_api.enabled` / `rest_api.port`) and documents the **MCP server**
  (`phoneme-mcp`, its five tools, and how to wire it into an MCP client). No more
  features that only existed in `config.toml`.
- [x] **Whole-library backup zip** — Settings → Storage → "Back up to .zip…"
  writes the same portable catalog+audio archive as `phoneme export <file>`
  (the old text-only Export is relabeled).

### CLI parity

- [x] **CLI reaches the GUI's per-recording actions** — `phoneme edit <id>
  --title/--clear-title/--favorite/--unfavorite`, `phoneme speaker
  rename|clear <id> <label> [name]`, `phoneme tag suggestions <id>
  [--approve|--dismiss <name>]`, `phoneme record pause/resume`, and
  `phoneme suggest-tags <id>` — all sending IPC requests that already
  existed, now reachable from the terminal.
- [x] **⚠️ Breaking: `phoneme record` controls are now subcommands** — `record
  start`, `record stop`, `record toggle`, `record cancel`, `record pause`,
  `record resume` (with `--in-place` on start/toggle), matching `meeting`,
  `daemon`, `queue`, and every other multi-action command. `record` was the
  lone outlier that took these as flags. The old flag spellings (`record
  --start`, `--stop`, `--toggle`, `--cancel`, `--pause`, `--resume`) have been
  **removed** — update any hotkey bindings or scripts to the subcommands.
  `--oneshot`/`--duration` remain modifiers of the bare (blocking) `record`.

### Hardening (audit fixes)

- [x] **`phoneme config set` no longer writes secrets in plaintext** — `set`
  used to persist the hand-edited toml_edit document verbatim, so
  `phoneme config set whisper.api_key sk-live-…` landed cleartext, bypassing
  DPAPI. It now writes the serialized validated `Config`, which runs the secret
  serializer → the new key is stored `dpapi:v1:…` and pre-existing encrypted
  keys stay encrypted (`bin/phoneme/src/commands/config_cmd.rs`).
- [x] **Tray profile switch no longer panics** — the Profiles-submenu switch
  resolved the daemon bridge with `app.state::<Option<Bridge>>()`, but the only
  managed state is `BridgeSlot`; `state::<T>()` panics on an unmanaged type, so
  every tray profile switch silently crashed its task and never reloaded the
  daemon. It now peeks the managed `BridgeSlot` like the exit hook
  (`src-tauri/src/tray.rs`).
- [x] **Bridge retry no longer double-executes mutations** — the bridge's
  transparent reconnect-and-retry fired on *any* request error, including a
  lost reply after the daemon had already run the request — so a dropped pipe
  could duplicate a non-idempotent mutation (`ImportRecording` mints a fresh id
  per call). The silent retry is now gated to read-only/idempotent requests
  (`is_retry_safe`); mutations surface the transport error instead
  (`src-tauri/src/bridge.rs`).

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

### Hardening (audit fixes)

- [x] **Webhook SSRF guard covers CGNAT + NAT64** — the classifier now treats
  carrier-grade NAT space `100.64.0.0/10` (RFC 6598) as private, and decodes the
  NAT64 well-known prefix `64:ff9b::/96` (RFC 6052) to classify the embedded IPv4
  — so neither can smuggle an internal host (e.g. the cloud metadata endpoint)
  past the guard (`phoneme-core::webhook`).
- [x] **`config validate` catches a keyless cloud summary** — an auto-summary
  (`[summary] auto = true`) on a cloud provider with no API key anywhere (own
  field empty and nothing to inherit from `[llm_post_process]`) now fails at
  save/load, matching the existing auto-tag/title/preview guards, instead of only
  falling back at runtime (`phoneme-core::config`).
- [x] **Provider endpoints deduplicated** — every cloud provider's default
  STT base URL and LLM chat/generate URL lives in one new
  `phoneme-core::endpoints` module; the live transcription/LLM paths and the
  `doctor` diagnostics now import the same consts instead of carrying their own
  copies, so the doctor can never probe a different endpoint than a recording
  hits. Pure refactor, no behavior change.
- [x] **Crash-recovery sweeps stuck `Paused` rows** — a daemon that crashed while
  a recording was paused left the catalog row spinning forever (no live recorder,
  no inbox file); startup reconciliation now sweeps `Paused` alongside the other
  in-progress statuses and flips an orphaned one to `transcribe_failed`
  (`phoneme-daemon::reconcile`).
- [x] **Dead-code cleanup** — removed the never-referenced
  `WhisperSupervisorConfig` struct (the real injection seam is `run_with`),
  dropped a stale struct-level `#[allow(dead_code)]` on `AppState` (all fields are
  read now), and scoped the `ActiveRecording` allow to just its one unread field
  (`mode`).

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
- [x] **Saved searches persist in the catalog** — saved searches moved out of
  the webview's `localStorage` into the catalog's `saved_searches` table, so they
  survive a reinstall and can ride catalog sync later (`ListSavedSearches` /
  `UpsertSavedSearch` / `DeleteSavedSearch` IPC). The frontend keeps its sync API
  via an in-memory cache that lazy-loads from the daemon and writes through;
  existing `localStorage` saves migrate over once, then the old key is cleared.
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
