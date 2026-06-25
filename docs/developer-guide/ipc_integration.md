# 📡 IPC Integration Guide (Advanced Automation)

Phoneme provides a full CLI (`phoneme record start`, `phoneme list`, etc.) that you can use to automate the application. However, under the hood, the CLI is just a thin wrapper that talks to the Phoneme Daemon.

For developers, hackers, and power users who want the lowest-latency automation possible—or who want to build their own custom user interfaces on top of Phoneme's engine—you can integrate directly with Phoneme's Inter-Process Communication (IPC) layer.

This page is the wire contract for the daemon's named pipe — the surface every
other client sits on top of. If you don't need raw-pipe access, two thin bridges
expose a subset over friendlier transports: the
[Local REST API](rest_api.md) (HTTP/SSE over loopback) and the
[MCP server](mcp_server.md) (JSON-RPC over stdio for AI clients). The
[`phoneme` CLI](cli_reference.md) and the
[Hook system](plugins_and_hooks.md) also drive this same daemon.

## 🏛️ The IPC Architecture

The Phoneme Daemon acts as a headless, always-on engine. It exposes a single, unified interface over a local named pipe.

- **Windows Named Pipe**: `\\.\pipe\phoneme-daemon`
- **Protocol**: Newline-delimited JSON (NDJSON)

Because the protocol is just JSON over a standard pipe/socket, you can interact with Phoneme using Python, Node, Go, Rust, AutoHotkey, or even raw netcat.

## 🧵 The Wire Protocol

When you connect to the named pipe, you can send `Request` objects and you will receive `Response` objects.

### 📤 Sending a Request

Requests must be a single line of JSON ending with a newline `\n`. They must include a `"type"` field indicating the command.

**Example Request:**
```json
{"type": "record_start", "mode": "toggle"}
```

**Example Response (success):**
```json
{"status": "ok", "value": null}
```

**Example Response (error):**
```json
{"status": "err", "value": {"kind": "already_recording", "message": "a recording is already in progress"}}
```

The `Response` is adjacently tagged: a `status` of `"ok"` or `"err"`, with the
payload under `value`. On error, `value` is an `IpcError` with a machine-readable
`kind` (`already_recording`, `not_recording`, `not_found`, `invalid_config`,
`whisper_unreachable`, `whisper_timeout`, `hook_failed`, `daemon_not_running`,
`pipe_in_use`, `shutting_down`, `io`, `internal`) plus a human `message`.

### 🤝 Version handshake (optional)

The wire protocol carries an integer version (`PROTOCOL_VERSION`, currently `1`).
Additive changes — a new request variant, a new optional field — keep it stable;
it only bumps on a breaking revision. A client that wants to fail fast against an
incompatible daemon can send a handshake as its first request:

```json
{"type": "handshake", "protocol_version": 1}
```

The daemon replies with its own view:

```json
{"status": "ok", "value": {"protocol_version": 1, "app_version": "1.8.1", "compatible": true}}
```

`compatible` is simply whether the daemon's `protocol_version` equals the one you
sent. The handshake is optional and backwards-compatible: `protocol_version`
defaults to `0` ("unversioned — proceed"), so an older daemon that doesn't know
the variant, or a client that never sends it, still works. The bundled `phoneme`
CLI sends this on connect and treats only an explicit `compatible: false` as a
hard stop (prompting you to restart the daemon or update the CLI so both sides
match); anything else proceeds.

### 📋 Full Request Schema

Phoneme supports the commands below (all snake_case). This page is a **map**; the
**canonical, always-current contract is the rustdoc on the `Request` enum in
`crates/phoneme-ipc/src/schema.rs`** — every variant there documents its exact
payload, the success-`value` shape, the `DaemonEvent`s it emits, and which surfaces
(GUI / CLI / tray hotkeys) send it. Build it locally with `cargo doc -p phoneme-ipc
--open` and read `schema::Request`. We deliberately don't re-list per-field payloads
here, because those drift; the field names below are just enough to orient you.

**Recording control** (`record_start` requires a `mode`: `"hold"`, `"oneshot"`, or
`{ "duration": secs }`, and an optional `in_place` bool). `record_start` and
`record_toggle` also take three optional one-time overrides — `recipe_id` (run a
named Playbook recipe instead of the default for this recording), `whisper_model`
(transcribe it with a specific STT model), and `source`
(`"microphone"` / `"system_audio"`, the capture source for this single recording).
All three omitted = the global default recipe + configured model + the global
`[recording].source`; these are how Custom Hotkeys carry their per-binding recipe /
model / source. None of the three is ever written to global config: `recipe_id`
rides the `pending_recipe` ledger (claimed by `pipeline::run` → `resolve_recipe`,
same mechanism the re-run override below uses), `whisper_model` the
`pending_overrides` model-override ledger, and `source` is applied at recorder
start — the recording's `track` then records which source it actually used. `source`
is **ignored for meetings** (a meeting always records both tracks). On
`record_toggle` all three apply only to the **start** half (a toggle that stops the
active recording has nothing new to attach them to):

> **Per-app tone** (`[in_place].app_recipes`) needs **none** of these fields. When
> a `record_start` / `record_toggle` for an in-place dictation omits `recipe_id`,
> the daemon resolves the per-app map against the foreground app it captures at
> record start and seeds the *same* `pending_recipe` ledger itself — so a matched
> app runs its recipe with no extra IPC. A `recipe_id` the binding *did* send wins
> (it overwrites the per-app seed). See `InPlaceConfig::resolve_app_recipe`.

- `record_start` (`in_place`, `recipe_id`, `whisper_model`, `source` optional), `record_stop`, `record_cancel`, `record_pause`, `record_resume`
- `record_toggle` (`in_place`, `recipe_id`, `whisper_model`, `source` optional), `record_status`

**Meeting control:**
- `start_meeting`, `stop_meeting`, `meeting_toggle`

**Catalog & import:**
- `list_recordings` (with a `filter`), `kind_counts` (per-Library-kind totals for the sidebar badges), `get_recording`, `list_meeting`, `get_segments` (machine transcript segments with ms timing + speaker labels; empty list when none are stored)
- `get_meeting_digest` (`{ "meeting_id" }`) — the **whole-meeting digest**: one LLM synthesis across **all** of a meeting's tracks (mic + system together), distinct from a single track's `summary`. Ok = the digest DTO `{ meeting_id, digest, digest_model }` or `null` when none has been generated yet (a normal state, not `not_found`). The merged meeting view fetches it alongside `list_meeting`.
- `list_meeting_digests` — every stored meeting digest, the many-meetings sibling of `get_meeting_digest`. Ok = a JSON array (possibly empty) of the same `{ meeting_id, digest, digest_model }` DTO.
- `get_period_digest` (`{ "key" }`) — a **period digest**: one LLM rollup across **every** recording in a date window, distinct from `get_meeting_digest` (which is meeting-scoped). `key` is the stable id derived from the canonical `since`/`until` bounds. Ok = the digest DTO `{ key, label, since, until, digest, digest_model, source_count }` or `null` when none has been generated for that range yet.
- `list_period_digests` — every stored period digest, newest range first. Ok = a JSON array (possibly empty) of the same DTO. A pure read used by the digest panel's history and the library-backup export (period digests live in their own side table, not carried by `list_recordings`).
- `get_words` (machine transcript **words** — the finer per-word layer beneath `get_segments`; ordered JSON array of `{ idx, start_ms, end_ms, text, speaker, confidence }`, where `confidence` is a 0..1 per-word score or `null` when the provider gives none — whisper-family endpoints emit only segment-level logprobs, so only Deepgram/AssemblyAI populate it. `speaker` is the `[Speaker N]` label (or `null` when undiarized): Deepgram/AssemblyAI tag words from their own speaker labels, and local diarization now tags each word too — it assigns speakers per word off the diarizer's per-frame activation matrix rather than per whole segment. Empty list when none are stored. Fetched lazily by the word-level features — word↔waveform seek and confidence highlighting)
- `delete_recording` (`keep_audio` bool), `delete_session` (`{ "meeting_id", "keep_audio" }` — delete every track of a meeting as a unit), `import_recording` (`{ "path", "recipe_id"? }` — `.wav`/`.mp3`/`.m4a`/`.flac`; the optional one-time `recipe_id` runs the import through that Playbook recipe instead of the global `default`, resolved + validated like `record`'s — a `scope = Meeting` recipe is rejected (`invalid_config`), an unknown id degrades to `default`, omit/empty for default)
- `export_clip` (`{ "id", "start_ms", "end_ms", "out_path"? }`) — export a `[start_ms, end_ms)` slice of a recording's audio to a new WAV (frame-aligned, source format; `end_ms` clamped to the duration). Ok `{"path":"<written file>"}`; `invalid_config` for a bad range. `phoneme clip`.
- `reimport_from_disk` (`{ "dry_run"? }`) — scan the audio dir for `.wav` files whose RecordingId has no catalog row and re-link each (insert a `queued` row + enqueue). Strictly **non-destructive**. Ok `{"count":N}` (or `{"count":N,"paths":[...]}` when `dry_run`). `phoneme doctor --reimport`.
- `rebuild_catalog` — **destructive** in-process rebuild from disk: clears every recording row (losing transcripts/edits/tags/summaries), then re-imports every WAV as a fresh `Queued` recording. Refused while a recording/meeting is in flight. Ok `{"count":N}`. Settings → Doctor, behind a type-to-confirm.
- `list_saved_searches`, `upsert_saved_search`, `delete_saved_search`, and `run_saved_search` (`{ "id" }`) — execute a stored saved search server-side: the daemon parses the saved `filter_json` into a `ListFilter` and runs the same query as `list_recordings`, returning the same recordings array. `not_found` for an unknown id, `invalid_config` when the stored filter won't parse.
- `list_ai_activity` (`recording_id` optional, `limit`) — the persisted AI-activity log: completed streaming LLM sessions (cleanup/summary and their re-runs) with the exact prompt + response, newest first. Powers the 🧠 popout's history so it survives app restarts. `recording_id` filters to one recording; omit it for the whole library's recent activity. The daemon prunes the table to a bounded recent window.
- **Dictation re-grab history** (opt-in, off unless `[in_place].keep_history` is on; the daemon prunes to the newest 50 on every insert):
  - `list_dictation_history` (`{ "limit" }`) — recent in-place dictations (the **text as typed** at the cursor, no audio), newest first. Ok = a JSON array of `{ id, text, char_count, app, created_at }` (`app` = the focused app's exe stem at type time, or `null`). Empty when the feature was never enabled.
  - `regrab_dictation` (`{ "id", "mode"? }`) — re-insert a past dictation's text at the **current** cursor. `mode` is `"type"`/`"paste"`, or omit it to use `[in_place].type_mode`. Ok = `{}`; `not_found` for an unknown id. **It types wherever the caret is now** (the original window is long gone), so it is **not retry-safe** — a blind re-send after a lost reply would type the text twice into your document.
  - `delete_dictation_history` (`{ "id" }`) — forget one entry (unknown ids are a no-op). Ok = `{ "removed": bool }`.
  - `clear_dictation_history` — empty the whole history. Ok = `{ "removed": N }`.

**Transcript & metadata edits:**
- `update_transcript`, `update_notes`, `update_meeting_name`
- `find_replace` (`{ "id", "find", "replace", "case_sensitive"? }`) — **literal** (not regex) find-and-replace across the live transcript, case-insensitive by default. Only the live `transcript` is rewritten (the preserved original/clean copies stay, so the edit is revertible); the word/segment timing layers are re-flowed and the text re-embedded exactly like `update_transcript`. A zero-match (or empty `find`) is a no-op. Ok = `{ "replaced": N }`; emits `transcript_updated` only when `N > 0`.
- `find_replace_library` (`{ "find", "replace", "case_sensitive"? }`) — the across-**all**-recordings counterpart of `find_replace`. Runs the same literal, revertible, timing-re-flowing replacement over every recording's live transcript in one request. A recording with zero matches is skipped entirely (no write, no version churn, no event); an empty `find` is a whole-operation no-op. Ok = `{ "recordings_changed": R, "total_replacements": N, "failed": F }` (F = recordings whose update errored, excluding the benign no-transcript skip; the sweep is best-effort and never aborts on one bad row); emits one `transcript_updated` per changed recording.
- `get_original_transcript` (raw machine transcript), `get_clean_transcript` (cleaned, pre-edit)
- `set_favorite` (star/unstar), `set_pinned` (pin/unpin — pinned recordings sort to the top of the library, independent of favorites), `set_speaker_name` (rename a diarized `[Speaker N]` label; never rewrites the stored transcript)
- **In-recording speaker correction** (fix the diarizer's per-segment assignments — `transcript_segments` stays authoritative, and each op rebuilds the prose transcript's `[Speaker N]:` markers in the same transaction so every view agrees; all three are mutating, not retry-safe, and emit `speaker_name_updated`):
  - `reassign_segment_speaker` (`{ "id", "idx": 0-based segment index, "new_label": 1-based label }`) — move one segment to another speaker; a brand-new label simply starts existing.
  - `merge_speakers` (`{ "id", "from_label", "into_label" }`) — every `from` segment becomes `into`, then `from` ceases to exist. `into` keeps its name (adopts `from`'s only when unnamed); `from`'s captured voiceprint is dropped (the centroid is per-label — a retranscribe re-captures the merged label) and any affected named voice is recomputed.
  - `split_speaker` (`{ "id", "label", "segment_idxs": [0-based, …], "new_label" }`) — move the listed segments from `label` onto a fresh `new_label` (no name/voiceprint until enrolled). An unknown idx, or one not currently `label`, aborts the whole op with no write.
- `set_recording_title` (`{ "id", "title": string|null }`) — set a display title; a non-null title is marked **user-owned** so auto-generation never overwrites it, while `null`/empty clears back to auto (regenerated on the next pipeline run). Emits the same `transcript_updated` refresh event edits use.

**Named-speaker recognition** (match diarized speakers against an enrolled voice library):
- `recognize_speakers` (`{ "id" }`) — on-demand: the still-unnamed diarized speakers whose voiceprints match a known voice. Ok = a JSON array of `SpeakerSuggestion` (empty when recognition is off or nothing matches).
- `dismiss_speaker_suggestion` (`{ "id", "speaker_label": 1-based }`) — suppress a suggestion so it isn't offered again for that recording + speaker. Ok = `{}`.
- `list_named_voices` — the named-voice library (id, name, sample count per enrolled voice). Ok = a JSON array of `NamedVoice`.
- `rename_named_voice` (`{ "id", "name" }`) — rename one enrolled voice. Ok = `{}`.
- `merge_named_voices` (`{ "from_id", "into_id" }`) — re-point the source's samples onto the target and delete the source. Ok = `{"merged":bool}`.
- `forget_named_voice` (`{ "id" }`) — **reversibly** soft-delete a voice (it vanishes from `list_named_voices` + recognition and its captures are unlinked; the raw per-recording voiceprints stay). Ok = `{"removed":bool}`.
- `undo_forget_named_voice` (`{ "id" }`) — undo a `forget_named_voice`: un-soft-delete the voice, re-link the captures it unlinked (skipping any re-named onto another voice since), recompute its centroid. Ok = `{"restored":bool}`.

**Chapters (LLM auto-chapters with timing):**
- `suggest_chapters` (`{ "id" }`) — on-demand run of the chapter step for one recording (regardless of recipe membership). Awaits the model; the time-ranged chapters land on the recording **replacing** any previous set, and `chapters_updated` fires (or `chapters_failed`). A recording with no transcript *segments* (no timing to chapter) is a clean no-op, not an error.
- `get_chapters` (`{ "id" }`) — fetch one recording's chapters in chronological order. Ok = a JSON array (possibly empty) of `Chapter` objects (`start_ms`/`end_ms`, `title`, optional `summary`). An empty list — including for an unknown id — is a normal state, not `not_found` (mirrors `get_segments`).

**Transcript versions** (the compounding chain — raw ASR at `idx` 0, then each Transform step's output; powers the Compare-versions view):
- `list_transcript_versions` (`{ "id" }`) — Ok = a JSON array of `{ idx, step_id, label, model, text }` in `idx` order, empty for a recording that ran no Transform.
- `get_transcript_version` (`{ "id", "idx" }`) — one version by step `idx` (`0` = raw ASR). Ok = the version object or `null` when absent.
- `revert_to_version` (`{ "id", "idx" }`) — revert the live transcript to a recorded version's text, through the same path as a manual edit (re-flows the timing variants + re-embeds). Ok `null`; emits `transcript_updated`; `not_found` when the recording or that version is missing.

**Tag suggestions (LLM auto-tag):**
- `suggest_tags` (on-demand suggest for one recording), `approve_tag_suggestion`, `dismiss_tag_suggestion`, `clear_all_tag_suggestions` (library-wide bulk clear)

**Entity extraction (LLM structured entities) + manual CRUD:**
- `suggest_entities` (`{ "id" }`) — on-demand run of the entity-extraction step for one recording (regardless of recipe membership). Awaits the model like `suggest_tags`; the typed entities (`person` / `org` / `topic` / `term`) land on the recording, **replacing** any previous set, and `entities_updated` fires (or `entities_failed`). The `Recording` DTO carries them as `entities: [{kind, value}, …]` plus the `entities_model` provenance field.
- `get_entities` (`{ "id" }`) — fetch one recording's entities (kind- then value-sorted) without pulling the whole `get_recording` row. Ok = a JSON array (possibly empty) of `{kind, value}`; an unknown id yields an empty list (mirrors `get_chapters`). The cross-recording facet is `list_all_entities` (below).
- `add_entity` (`{ "id", "kind", "value" }`) — add a user-curated entity. Manual entities survive re-extraction (only LLM rows are replaced). Ok `null`; emits `entities_updated`.
- `update_entity` (`{ "id", "kind", "value", "new_kind", "new_value" }`) — edit one entity in place, keyed by its current `(kind, value)`; marks it manual so the fix survives re-extraction. Ok `null`; emits `entities_updated`.
- `delete_entity` (`{ "id", "kind", "value" }`) — delete one entity from a recording, keyed by `(kind, value)`. Ok `null`; emits `entities_updated`.
- `merge_entities` (`{ "kind", "from_values": [...], "to_value" }`) — library-wide: fold every `from_values` entity of `kind` into `to_value` across all recordings. Ok `null`; emits `entities_merged`.

**Task extraction (LLM action items, with a mutable done flag) + manual CRUD:**
- `suggest_tasks` (`{ "id" }`) — on-demand run of the task-extraction step for one recording (regardless of recipe membership). Awaits the model like `suggest_entities`; the action items (`{text, due_hint?}`) land on the recording **replacing** the previous set, **but any `done` flag the user set is preserved** when a task's text survives, and `tasks_updated` fires (or `tasks_failed`). The `Recording` DTO carries them as `tasks: [{id, text, due_hint, done}, …]` (open first) plus the `tasks_model` provenance field. `due_hint` is the model's free-text deadline phrase verbatim — never a parsed date.
- `set_task_done` (`{ "id", "task_id", "done" }`) — toggle one task's `done` flag (the one entities lack). Emits `tasks_updated`; `not_found` when `task_id` matches no row.
- `add_task` (`{ "id", "text", "due_hint"? }`) — add a user-created task. Manual tasks survive re-extraction. Ok `null`; emits `tasks_updated`.
- `update_task` (`{ "id", "task_id", "text", "due_hint"? }`) — edit one task's text/due hint. Ok `null`; emits `tasks_updated`; `not_found` for an unknown `task_id`.
- `delete_task` (`{ "id", "task_id" }`) — delete one task. Ok `null`; emits `tasks_updated`; `not_found` for an unknown `task_id`.
- `reorder_tasks` (`{ "id", "task_ids": [...] }`) — set the user's task order (each id's position becomes its `sort_order`); ids not in the recording are ignored. Ok `null`; emits `tasks_updated`.

The `list_recordings` filter takes `limit`/`offset` (pagination),
`since`/`until` (RFC 3339), `status` (one of the recording statuses below),
`search` (FTS5), `tag_id`, `sort_desc`, plus the type filters applied in SQL
**before** pagination so pages stay full: `kind` (`"single"` voice notes /
`"meeting"` tracks; omit for all), `favorite` (`true` = starred only,
`false` = unstarred only), `pinned` (`true` = pinned only, `false` = unpinned
only), and `in_place` (`true` = only in-place-dictation recordings). It also
takes `tagged` (`true` = recordings with ≥1 tag, `false` = untagged) and the
**entity facet filter** `entity_value` (+ optional `entity_kind`) — keep only
recordings that mention this extracted entity, via a `recordings.id IN (SELECT
recording_id FROM entities WHERE value = ? [AND kind = ?])` subquery, the entity
counterpart of `tag_id`. It also takes the **task-presence filter** `task_state`
(`"has_open"` = recordings with ≥1 not-done task, `"has_tasks"` = any extracted
task) via a `recordings.id IN (SELECT recording_id FROM tasks [WHERE done = 0])`
subquery; an unrecognized value is ignored. All fields are optional; older clients
that omit the newer ones keep working. `list` always sorts pinned recordings first
(`pinned DESC` leads the ORDER BY), ahead of the date sort, so pins float to the
top regardless of `sort_desc`.

`kind_counts` returns full-corpus recording counts per Library kind as a JSON
object — `{all, single, meeting, in_place, favorite, pinned, tagged, untagged}`
(one SQL pass, `Catalog::kind_counts`) — powering the sidebar's Library count
badges.

`list_all_entities` returns the **cross-recording entity facet**: every distinct
extracted entity across the library with its recording count, as a JSON array of
`{kind, value, count}`, kind- then value-sorted (`Catalog::entity_facets`). It is
the entity counterpart of `list_all_tags` + `tag_usage_counts`, powers the
sidebar's browse-by-entity surface and `phoneme entities`, and pairs with the
`entity_value` / `entity_kind` list filter above to drill from a facet row into
the recordings that mention it.

`list_all_tasks` (`{ "only_open" }`, default `false`) returns the **cross-recording
task list**: every extracted task across the library as a JSON array of
`{recording_id, title, id, text, due_hint, done}`, open first then newest recording
first (`Catalog::list_all_tasks`). It is the task counterpart of `list_all_entities`,
powers the sidebar's Tasks section and `phoneme tasks`, and pairs with the
`task_state` list filter above. When `only_open` is `true`, done tasks are dropped.

`task_counts` returns the cheap badge counts the sidebar's Tasks section needs
without pulling the full `list_all_tasks` payload (see
`phoneme_core::types::TaskCounts`).

Recording `status` values: `recording`, `paused`, `queued`, `transcribing`,
`cleaning_up`, `summarizing`, `tagging`, `hook_running`, `done`,
`transcribe_failed`, `hook_failed`, `cleanup_failed`, `summarize_failed`,
`title_failed`, `tag_failed`, and `cancelled`. `queued` is the recording
**waiting** in the serial transcription queue — it flips to `transcribing` only
when the worker actually claims it (enqueue sets `queued`, so a waiting item is
no longer mislabelled `transcribing`). The four optional-step failures
(`cleanup_failed` / `summarize_failed` / `title_failed` / `tag_failed`) are
terminal like `hook_failed`: the transcript is intact and the recording is
fully usable — only that enrichment step failed — and the reason is persisted on
the row (`error_kind` = the status, `error_message` = why), so the failure is
filterable, searchable, and survives a restart. `cancelled` is terminal like the
failures but means the **user** stopped the run (`cancel_queued`,
`cancel_all_queued`, or `cancel_processing`) — clients should never render it
as a failure.

**Re-processing** (one-time overrides, never persisted to config):
- `retranscribe_recording` (optional `model`, `run_hooks`, `post_process`, `recipe_id`). `recipe_id` re-runs the recording through any named Playbook recipe (empty/omitted = the global `default` recipe); it is stashed in the `pending_recipe` ledger for this job only and claimed by `pipeline::run` → `resolve_recipe` — the **same** one-time mechanism a custom hotkey's recipe uses, never persisted. The Re-run modal's per-step model tabs layer on top as separate one-time overrides.
- `rerun_cleanup` (re-runs only LLM cleanup against the preserved original; optional `model`/`provider`/`prompt`/`api_url`/`api_key`)
- `rerun_summary` (generate/regenerate an LLM summary; optional `model`/`prompt`)
- `rerun_meeting_digest` (`{ "meeting_id", "model"?, "recipe_id"? }`) — generate/regenerate the **whole-meeting digest** (one LLM synthesis across all of a meeting's tracks), the meeting-scope twin of `rerun_summary`. Runs the configured **meeting template** (a `scope = meeting` recipe, `meeting_recipe_id` — empty = the built-in digest prompt) over the merged meeting transcript, reusing the summary provider. `model` overrides the summary model for this run only; `recipe_id` runs a specific meeting template for this run only (an unknown or non-meeting-scope id falls back to the built-in digest, never an error). Acks `null` immediately and runs detached, emitting `pipeline_stage_changed` + `llm_activity` (keyed on the meeting's first track) and finally `meeting_digest_updated` (or `meeting_digest_failed`). Errors up front: `not_found` for an unknown meeting, no transcribed tracks, or `invalid_config` when no usable summary LLM provider is configured. A digest is **also** generated automatically when a meeting finalizes (both tracks done), gated on `[summary].auto`, using the same configured template.
- `rerun_period_digest` (`{ "since", "until", "label", "model"? }`) — generate/regenerate a **period digest** (one LLM rollup across every recording in the `since`..`until` window), the date-window twin of `rerun_meeting_digest`. The daemon selects the window's recordings (`list_recordings` filter, oldest-first), concatenates their transcripts (each prefixed with its date + title, capped to a size limit), and runs the merged text through the configured summary provider; `model` overrides the summary model for this run only. The result is stored keyed by the range (re-running the same window upserts in place). Acks `null` immediately and runs detached, emitting `pipeline_stage_changed` + `llm_activity` (keyed on the window's first recording) and finally `period_digest_updated` (or `period_digest_failed`). Errors up front: `not_found` for a window with no recordings, no transcribed recordings, or `invalid_config` when no usable summary LLM provider is configured. **Single-attempt** like the other re-runs — a blind re-send could fire the LLM twice.
- `refire_hook` (optional `command`, restricted to the configured allowlist)

**Pipeline & preview control:**
- `restart_whisper` (force-restart the bundled whisper-server(s); the Doctor's "Fix" for an unreachable local Whisper)
- `skip_current_stage` (skip the LLM stage currently running for the active queue item — the pipeline continues as if that stage failed non-fatally)
- `set_preview_source` (`track`: switch which meeting track feeds the live preview)

**Queue (inbox) operations:** inspect and manage the durable inbox the queue worker drains.
- `list_queue` (processing item(s) first, then pending in claim order), `queue_counts` (`{pending, processing, done, failed}`)
- `cancel_queued` (drop one pending item → marks it `cancelled`), `cancel_all_queued` (drop every pending item), `cancel_processing` (abort the in-flight item)
- `reorder_queue` (`ids`: desired claim order), `set_queue_paused` (`paused` bool), `queue_paused` (query)
- `clear_failed` (empty the `failed/` quarantine), `dismiss_failed` (`{ "id" }` — remove one quarantined `failed/<id>.json` payload; the per-item counterpart to `clear_failed`, leaving the catalog row untouched)

**Tags:** `list_tags`, `list_all_tags`, `add_tag`, `update_tag`, `delete_tag`,
`attach_tag`, `detach_tag`, `tags_for`, `tag_usage_counts`, `merge_tags`.

**Search / recall:** `semantic_search` (`query`, `limit`, optional `filter`) — a
`filter` (the same `ListFilter` shape as `list_recordings`) scopes the meaning-
search to matching recordings (tag/status/date/kind/…), applied after ranking and
before the limit; omit it for an unscoped search; `more_like_this` (`id`,
`limit`) — "more like this": ranks the library by similarity to a stored
recording using its already-stored vectors (no fresh embedding), excluding the
source itself and the other track of its own meeting. Both respond with the
same `[{ "recording": …, "score": … }]` array (calibrated 0..1 scores);
`more_like_this` errors with a clear "isn't indexed yet" message when the
source recording has no embeddings. `reembed_all` clears and rebuilds every
stored embedding with the current model (use after changing the embedding model).

`ask` (`request_id`, `query`, optional `top_k`, optional `filter`) — **Ask my
archive** (local RAG): answers `query` grounded **only** in the user's
transcripts, with citations. It rides the *same* hybrid retriever as
`semantic_search` (so `filter` scopes it identically), then streams an LLM answer
through the configured `[llm_post_process]` provider. The client mints
`request_id` and **subscribes first** (on a second connection — see Event
Streaming), because the request acknowledges immediately with `null` and the work
streams asynchronously over `ask_activity` events tagged with that `request_id`.
A synchronous `err` (`invalid_config`) means no embedder is loaded or no LLM
provider is configured; a failure *after* the ack (embed / retrieval /
generation) instead arrives as a terminal `ask_activity` with `error` set. Empty
retrieval returns a terminal "nothing matched" answer **without** calling the LLM.

**Diagnostics:** `run_doctor` (runs all health checks; the GUI Doctor view).
`export_diagnostics` writes an opt-in, **local-only** sanitized bundle for bug
reports — app/version/OS info, the *masked* config (every secret redacted; never
a plaintext key), and a tail of the daemon log — to
`<data_dir>/diagnostics/phoneme-diagnostics-<timestamp>.json`, and returns
`{"path":"<written file>"}`. It makes no network calls and includes no
audio/transcripts/catalog data (the GUI Doctor's "Export diagnostics" button).

**Daemon control:** `daemon_status`, `reload_config`, `shutdown`, `hook_test`,
`subscribe_events` (see Event Streaming below).

`daemon_status` answers `running`/`pid`/`version` plus three bundled
whisper-server port pairs: the main server (`whisper_preferred_port` /
`whisper_effective_port`), the optional `[preview_whisper]` server
(`preview_whisper_*`), and the optional dedicated `[in_place.stt]` dictation
server (`dictation_whisper_*`). *Preferred* is the configured port (`null` when
that server isn't configured); *effective* is the port the server is actually
listening on — the daemon falls back to a free port when the preferred one is
held by another app, and reports `null` while that server isn't running.
Anything probing the local server should dial the effective port when present.

`shutdown` acknowledges **before** the daemon exits: the `{"status":"ok"}`
response is written to the pipe first, and the actual teardown begins a
fraction of a second later — so a client always gets its reply instead of a
broken pipe. The teardown then stops and queues any in-flight recording, kills
the daemon-spawned whisper-server(s) and a daemon-launched Ollama, and exits.
Expect the pipe to disappear shortly after the reply; reconnect attempts
should treat that as success, the way `phoneme daemon stop` does.

## 🌊 Real-Time Event Streaming

The IPC layer's real-time event stream is what lets you build a live UI on top of the daemon. By sending the `subscribe_events` request, the daemon holds the connection open and pushes live events to your application as they happen.

**Send:**
```json
{"type": "subscribe_events"}
```

Events are **internally tagged**: each event is a flat object with an `event`
field naming the variant, plus that variant's fields alongside it.

**Stream Received:**
```json
{"event": "recording_started", "id": "20260519T143500823", "started_at": "2026-05-19T14:35:00.823-07:00", "meeting_id": null, "track": null}
{"event": "transcription_started", "id": "20260519T143500823"}
{"event": "transcription_partial", "id": "20260519T143500823", "text": "Hello, this is a live preview..."}
{"event": "recording_stopped", "id": "20260519T143500823", "duration_ms": 4200, "audio_path": "...", "meeting_id": null}
{"event": "pipeline_stage_changed", "id": "20260519T143500823", "stage": "transcribing"}
{"event": "queue_depth_changed", "pending": 1, "processing": 0, "failed": 0}
{"event": "transcription_done", "id": "20260519T143500823", "transcript": "Hello, this is a live preview."}
{"event": "summary_updated", "id": "20260519T143500823"}
{"event": "meeting_digest_updated", "meeting_id": "meeting-20260519T143500823"}
```

The whole-meeting digest emits `meeting_digest_updated` (success) or
`meeting_digest_failed` (`{ meeting_id, error }`) — the meeting-scope twins of
`summary_updated` / `summary_failed`, keyed by `meeting_id` rather than a
recording `id`.

Both the summary and the digest also stream while they generate: their
`summarizing`-stage `llm_activity` events (keyed on the recording `id`, or the
meeting's **first track** `id` for a digest) carry the prompt then incremental
`delta`s, so the GUI's summary peek and meeting-digest card render the text token
by token, then settle to the full stored text on the `*_updated` event. The
stream is display-only (the daemon caps it at `MAX_STREAMED_CHARS`); the
`*_updated` result is authoritative.

The period digest emits `period_digest_updated` (`{ key }`, success) or
`period_digest_failed` (`{ key, error }`) — the date-window twins of the meeting
digest events, keyed by the range `key`.

Entity extraction emits `entities_updated` (`{ id }`) when the typed entities are
stored, or `entities_failed` (`{ id, error }`) on failure — the entity twins of
`tag_suggestions_updated` / `tag_failed`. A failure is best-effort: the
transcript stays intact and the recording usable; only the optional entity step
failed.

Task extraction emits `tasks_updated` (`{ id }`) when the action items are stored,
toggled (`set_task_done`), or re-extracted, or `tasks_failed` (`{ id, error }`) on
failure — the task twins of `entities_updated` / `entities_failed`, equally
best-effort. A library-wide `merge_entities` emits `entities_merged` so every
view that browses entities (the sidebar facet, the Entity manager, an open
recording's chips) refetches.

Chapter generation emits `chapters_updated` (`{ id }`) when the time-ranged
chapters are stored, or `chapters_failed` (`{ id, error }`) on failure — the
chapter twins of `entities_updated` / `entities_failed`. The Chapters view
re-fetches (`get_chapters`) on the success event.

**Ask my archive stream** (`ask_activity`, after sending `ask` with a
`request_id`): the daemon ships the citation `sources` first (before any token),
then `delta` chunks of the answer, then a terminal `done`. Filter by
`request_id`; an answer's inline `[n]` markers map to `sources[n-1]`.

```json
{"event": "ask_activity", "request_id": "ask-1", "sources": [{"n": 1, "recording_id": "20260519T143500823", "meeting_id": null, "label": "Standup notes", "chunk_index": 2, "snippet": "we deferred the migration…", "relevance": 0.71}], "delta": "", "done": false, "error": ""}
{"event": "ask_activity", "request_id": "ask-1", "sources": [], "delta": "The migration was deferred [1].", "done": false, "error": ""}
{"event": "ask_activity", "request_id": "ask-1", "sources": [], "delta": "", "done": true, "error": ""}
```

The full event catalog — recording lifecycle, `pipeline_stage_changed`,
`llm_activity` (streaming prompt/response chunks), `recording_cancelled`, the
tag/queue/speaker/meeting events — is the `DaemonEvent` enum in
`crates/phoneme-ipc/src/schema.rs`, where every variant documents its fields and
when it fires. Subscribe over a **separate** connection: a `subscribe_events`
connection never receives `Response`s, so a client that needs both events and
commands opens two pipes. A subscriber that falls behind the daemon's broadcast
buffer is disconnected and must reconnect and re-fetch state.

This is the same API the official Phoneme GUI uses to stay in sync. You can use it
to build custom overlays, status LEDs on hardware, or notification systems.

## ⌨️ Example: AutoHotkey Integration

If you want to trigger Phoneme instantly using a custom keyboard shortcut via AutoHotkey, you don't need to spin up the `phoneme.exe` CLI process. You can write directly to the pipe.

*(Note: While possible, AHK makes named pipes a bit tricky. Python or Node are generally easier for scripting!)*

## 🟢 Example: Node.js Integration

Here is a complete, working example of how to build a Node.js script that listens to Phoneme's live transcription events as you speak.

```javascript
const net = require('net');

const PIPE_NAME = '\\\\.\\pipe\\phoneme-daemon';
const client = net.createConnection(PIPE_NAME, () => {
    console.log('Connected to Phoneme Daemon!');
    
    // Subscribe to real-time events
    client.write(JSON.stringify({ type: "subscribe_events" }) + '\n');
});

client.on('data', (data) => {
    const lines = data.toString().split('\n').filter(Boolean);
    
    for (const line of lines) {
        try {
            const msg = JSON.parse(line);

            // Events are flat objects tagged by `event`; the variant's fields
            // sit alongside it (e.g. `text` for transcription_partial).
            if (msg.event === "transcription_partial") {
                console.log('Live Transcript:', msg.text);
            }
        } catch (e) {
            console.error('Failed to parse:', line);
        }
    }
});

client.on('end', () => console.log('Disconnected'));
```

## 🛡️ Security Notice

The named pipe `\\.\pipe\phoneme-daemon` is restricted by Windows OS-level security to the current user session. Other users on the same machine cannot connect to your Phoneme daemon. 

However, because it is unauthenticated over the pipe, any application running under your user account can trigger recordings or access your transcript catalog. This is standard for local desktop applications.
