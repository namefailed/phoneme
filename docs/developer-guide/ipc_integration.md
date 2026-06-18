# 📡 IPC Integration Guide (Advanced Automation)

Phoneme provides a full CLI (`phoneme record start`, `phoneme list`, etc.) that you can use to automate the application. However, under the hood, the CLI is just a thin wrapper that talks to the Phoneme Daemon.

For developers, hackers, and power users who want the lowest-latency automation possible—or who want to build their own custom user interfaces on top of Phoneme's engine—you can integrate directly with Phoneme's Inter-Process Communication (IPC) layer.

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
`record_toggle` also take optional `recipe_id` (run a named Playbook recipe instead
of the default for this recording) and `whisper_model` (transcribe it with a
specific STT model) — both omitted = the global default recipe + configured model;
these are how Custom Hotkeys carry their per-binding recipe/model:
- `record_start`, `record_stop`, `record_cancel`, `record_pause`, `record_resume`
- `record_toggle` (`in_place`, `recipe_id`, `whisper_model` optional), `record_status`

**Meeting control:**
- `start_meeting`, `stop_meeting`, `meeting_toggle`

**Catalog & import:**
- `list_recordings` (with a `filter`), `kind_counts` (per-Library-kind totals for the sidebar badges), `get_recording`, `list_meeting`, `get_segments` (machine transcript segments with ms timing + speaker labels; empty list when none are stored)
- `get_words` (machine transcript **words** — the finer per-word layer beneath `get_segments`; ordered JSON array of `{ idx, start_ms, end_ms, text, speaker, confidence }`, where `confidence` is a 0..1 per-word score or `null` when the provider gives none — whisper-family endpoints emit only segment-level logprobs, so only Deepgram/AssemblyAI populate it. `speaker` is the `[Speaker N]` label (or `null` when undiarized): Deepgram/AssemblyAI tag words from their own speaker labels, and local diarization now tags each word too — it assigns speakers per word off the diarizer's per-frame activation matrix rather than per whole segment. Empty list when none are stored. Fetched lazily by the word-level features — word↔waveform seek and confidence highlighting)
- `delete_recording` (`keep_audio` bool), `import_recording` (`.wav`/`.mp3`/`.m4a`/`.flac`)
- `list_ai_activity` (`recording_id` optional, `limit`) — the persisted AI-activity log: completed streaming LLM sessions (cleanup/summary and their re-runs) with the exact prompt + response, newest first. Powers the 🧠 popout's history so it survives app restarts. `recording_id` filters to one recording; omit it for the whole library's recent activity. The daemon prunes the table to a bounded recent window.

**Transcript & metadata edits:**
- `update_transcript`, `update_notes`, `update_meeting_name`
- `get_original_transcript` (raw machine transcript), `get_clean_transcript` (cleaned, pre-edit)
- `set_favorite` (star/unstar), `set_speaker_name` (rename a diarized `[Speaker N]` label; never rewrites the stored transcript)
- `set_recording_title` (`{ "id", "title": string|null }`) — set a display title; a non-null title is marked **user-owned** so auto-generation never overwrites it, while `null`/empty clears back to auto (regenerated on the next pipeline run). Emits the same `transcript_updated` refresh event edits use.

**Tag suggestions (LLM auto-tag):**
- `suggest_tags` (on-demand suggest for one recording), `approve_tag_suggestion`, `dismiss_tag_suggestion`, `clear_all_tag_suggestions` (library-wide bulk clear)

The `list_recordings` filter takes `limit`/`offset` (pagination),
`since`/`until` (RFC 3339), `status` (one of the recording statuses below),
`search` (FTS5), `tag_id`, `sort_desc`, plus three type filters applied in SQL
**before** pagination so pages stay full: `kind` (`"single"` voice notes /
`"meeting"` tracks; omit for all), `favorite` (`true` = starred only,
`false` = unstarred only), and `in_place` (`true` = only in-place-dictation
recordings). All fields are optional; older clients that omit the newer ones
keep working.

`kind_counts` returns full-corpus recording counts per Library kind as a JSON
object — `{all, single, meeting, in_place, favorite}` (one SQL pass,
`Catalog::kind_counts`) — powering the sidebar's Library count badges.

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
- `retranscribe_recording` (optional `model`, `run_hooks`, `post_process`)
- `rerun_cleanup` (re-runs only LLM cleanup against the preserved original; optional `model`/`provider`/`prompt`/`api_url`/`api_key`)
- `rerun_summary` (generate/regenerate an LLM summary; optional `model`/`prompt`)
- `refire_hook` (optional `command`, restricted to the configured allowlist)

**Pipeline & preview control:**
- `restart_whisper` (force-restart the bundled whisper-server(s); the Doctor's "Fix" for an unreachable local Whisper)
- `skip_current_stage` (skip the LLM stage currently running for the active queue item — the pipeline continues as if that stage failed non-fatally)
- `set_preview_source` (`track`: switch which meeting track feeds the live preview)

**Queue (inbox) operations:** inspect and manage the durable inbox the queue worker drains.
- `list_queue` (processing item(s) first, then pending in claim order), `queue_counts` (`{pending, processing, done, failed}`)
- `cancel_queued` (drop one pending item → marks it `cancelled`), `cancel_all_queued` (drop every pending item), `cancel_processing` (abort the in-flight item)
- `reorder_queue` (`ids`: desired claim order), `set_queue_paused` (`paused` bool), `queue_paused` (query), `clear_failed` (empty the `failed/` quarantine)

**Tags:** `list_tags`, `list_all_tags`, `add_tag`, `update_tag`, `delete_tag`,
`attach_tag`, `detach_tag`, `tags_for`, `tag_usage_counts`, `merge_tags`.

**Search / recall:** `semantic_search` (`query`, `limit`); `more_like_this` (`id`,
`limit`) — "more like this": ranks the library by similarity to a stored
recording using its already-stored vectors (no fresh embedding), excluding the
source itself and the other track of its own meeting. Both respond with the
same `[{ "recording": …, "score": … }]` array (calibrated 0..1 scores);
`more_like_this` errors with a clear "isn't indexed yet" message when the
source recording has no embeddings. `reembed_all` clears and rebuilds every
stored embedding with the current model (use after changing the embedding model).

**Diagnostics:** `run_doctor` (runs all health checks; the GUI Doctor view).

**Daemon control:** `daemon_status`, `reload_config`, `shutdown`, `hook_test`,
`subscribe_events` (see Event Streaming below).

`daemon_status` answers `running`/`pid`/`version` plus the bundled
whisper-server ports: `whisper_preferred_port` / `whisper_effective_port` and
the `preview_whisper_*` pair. *Preferred* is the configured
`bundled_server_port`; *effective* is the port the server is actually
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

The most powerful feature of the IPC layer is real-time event streaming. By sending the `subscribe_events` request, the daemon will hold the connection open and push live events to your application as they happen.

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
