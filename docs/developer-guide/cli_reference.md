# 💻 Phoneme CLI Reference

Every core action in Phoneme is fully accessible from the command line interface via `phoneme.exe` (the client) and `phoneme-daemon.exe` (the engine).

## 🌐 Global flags

These apply to any subcommand:

| Flag | Effect |
|------|--------|
| `--json` | JSON-lines output where supported |
| `--no-color` | Disable colored output (or set `NO_COLOR=1`) |
| `-v`, `--verbose` | Verbose tracing to stderr |

## 🚦 Spawn vs observe

The CLI auto-spawns the daemon when needed — but only for commands that
**create work**. Read-only / inspection commands never start a daemon: a
daemon-is-down state is itself the answer for them, so they print
`daemon not reachable` and exit with code 3 instead of silently starting one.

| Behavior | Commands |
|----------|----------|
| **Auto-spawn** (start the daemon if it's not running, then send) | `record`, `meeting start/stop/toggle/rename`, `import`, `retranscribe`, `cleanup`, `summarize`, `suggest-tags`, `notes`, `edit`, `find-replace`, `clip`, `speaker rename/clear/reassign/merge/split`, `reembed`, `refire-hook`, `delete`, `queue pause/resume/reorder/cancel/cancel-processing/cancel-all/clear-failed/dismiss-failed`, `tag add/update/delete/attach/detach/clear-suggestions/merge`, `profile use`, `hook test`, `export` (zip and `--captions`), `config reload`, `daemon start` |
| **Observe-only** (fail fast with exit 3 when no daemon) | `list`, `show`, `search`, `watch`, `doctor`, `daemon status`, `queue list/counts/status`, `queue skip`*, `tag list/for/usage`, `meeting tracks`, `profile list` |
| **Purely local** (no daemon involved at all) | `config` (print), `config path`, `config set`, `profile save`, `version` |

\* `queue skip` mutates, but only a live daemon mid-LLM-stage has anything to
skip — spawning one just to skip nothing would mask reality.

`daemon stop` is its own special case: it stops a running daemon but never
spawns one just to stop it (stopping an already-stopped daemon succeeds).

## 🚪 Exit codes

Exit codes are stable API — scripts can branch on them. Every command maps a
daemon error to the same code via one shared table:

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Generic failure (including internal daemon errors) |
| `2` | Usage error (bad flags — clap's own) |
| `3` | Daemon not reachable (also: pipe in use, daemon shutting down) |
| `4` | Whisper backend unreachable or timed out |
| `5` | Hook failed |
| `6` | Invalid config (e.g. a rejected `config set` value) |
| `7` | Recording / tag / path not found |

## ⚙️ Core Commands

### 🎤 `phoneme record`

Start, stop, or run a one-shot recording. The non-blocking controls are
**subcommands** (`record start`, `record stop`, …), matching `meeting` and the
rest of the CLI; bare `phoneme record` (no subcommand) is the blocking
push-to-talk mode.

```bash
# Non-blocking: starts the recording and immediately returns.
phoneme record start

# Non-blocking: stops the current recording and begins transcription/hooks.
phoneme record stop

# Non-blocking: start if idle, otherwise stop the active recording (atomic —
# ideal for a single hotkey binding). Takes --in-place.
phoneme record toggle

# In-Place Mode: the transcript is typed out as simulated keystrokes into the
# currently focused application window. `-i` is the short form of `--in-place`
# (valid on bare `record`, `record start`, and `record toggle`).
phoneme record start --in-place

# Discard the active recording without saving.
phoneme record cancel

# Non-blocking: pause / resume the active recording (or every track of the
# active meeting). Exit 0.
phoneme record pause
phoneme record resume

# Blocking: starts recording, waits for you to press Enter (or timeout),
# then stops, transcribes, and prints the result.
phoneme record --oneshot

# Record exactly 10 seconds (blocking).
phoneme record --duration 10

# Run a specific Playbook recipe (by id or name) instead of the default
# pipeline. Works on the blocking default, `record start`, and `record toggle`.
phoneme record --recipe meeting_notes
phoneme record start --recipe "Meeting notes"
phoneme record toggle --recipe prompt_capture
```

Each non-blocking subcommand sends a single request (`RecordStart`,
`RecordStop`, `RecordToggle`, `RecordCancel`, `RecordPause`, `RecordResume`) and
exits 0. `--oneshot` / `--duration` modify the blocking default.

> **`--recipe <ID|NAME>`:** pick a Playbook recipe like the GUI's recipe
> picker. The value is matched against your configured recipes — by id first,
> then case-insensitively by name — and the resolved id is sent to the daemon.
> Omit it for the default pipeline. A value that matches no recipe is an error
> (it lists the available recipes); it never silently falls back to default.
> Available on bare `phoneme record` (blocking / `--oneshot` / `--duration`),
> `record start`, and `record toggle`.

> **Breaking change:** the pre-1.8 flag spellings (`record --start`, `--stop`,
> `--toggle`, `--cancel`, `--pause`, `--resume`) were **removed** — use the
> subcommands. Update any hotkey bindings or scripts accordingly.

> **Capture source:** `phoneme record` always records the **global**
> `[recording].source` (microphone by default; `system_audio` for WASAPI
> loopback) — there's **no `--source` flag**. The per-keybind capture-source
> override (`[[hotkeys]].source`) is a **GUI/config-only** feature for custom
> hotkeys, not the CLI; set it in **Settings → Hotkeys** or in `config.toml`.
> See the [config reference](config_reference.md).

### 👥 `phoneme meeting`

Start a dual-track Meeting Mode recording.

```bash
# Start capturing mic + system audio
phoneme meeting start

# Stop the meeting and transcribe both tracks
phoneme meeting stop

# Start if no meeting is active, otherwise stop it (atomic, for hotkey bindings)
phoneme meeting toggle

# List every recording (track) belonging to a meeting session
phoneme meeting tracks 20260519T143500823

# Rename a meeting
phoneme meeting rename 20260519T143500823 "Q3 Planning Sync"

# Clear a meeting's name (omit NAME and pass --clear) — reverts to the
# auto-generated label.
phoneme meeting rename 20260519T143500823 --clear
```

### 📥 `phoneme import <FILE-OR-URL>`

Import an existing audio file (wav/mp3/m4a/flac) and transcribe it — or pass an
`http(s)` URL (e.g. a YouTube link) to download its audio with **yt-dlp** and
import that.

```bash
phoneme import my_meeting.mp3

# From a URL — downloads audio-only via yt-dlp, then imports it
phoneme import "https://www.youtube.com/watch?v=VIDEO_ID"

# Choose the extracted format for URL imports (default m4a)
phoneme import --format flac "https://youtu.be/VIDEO_ID"
```

| Flag | Default | Notes |
| --- | --- | --- |
| `--format <m4a\|mp3\|flac\|wav>` | `m4a` | Audio format yt-dlp extracts to (URL imports only). m4a/mp3 are lossy but transparent for speech; flac/wav avoid a re-encode. |

URL import requires **yt-dlp** and **ffmpeg** on PATH (`python -m pip install -U
yt-dlp`). The download lands in a temp folder and is removed after import —
Phoneme keeps only its own decoded copy. Pair it with `retranscribe` to A/B
transcription settings on real-world audio.

### 📋 `phoneme list`

Query the local SQLite recording catalog.

```bash
# List all recordings
phoneme list

# List recordings in a date range (ISO 8601, both bounds inclusive)
phoneme list --since 2026-05-19
phoneme list --since 2026-05-01 --until 2026-05-31

# Filter by status: recording, paused, queued, transcribing, cleaning_up,
# summarizing, tagging, hook_running, done, transcribe_failed, hook_failed,
# cleanup_failed, summarize_failed, title_failed, tag_failed, or cancelled.
#  - queued: waiting in the transcription queue (flips to transcribing when the
#    worker claims it) — so a recording that's only waiting no longer reads as
#    "transcribing".
#  - *_failed for an optional step (cleanup/summary/title/tag) is terminal like
#    hook_failed: the transcript is intact, only that enrichment didn't land, and
#    the reason is stored on the row so you can find and re-run it.
#  - cancelled: a run the user cancelled — terminal, but not a failure.
phoneme list --status done
phoneme list --status cancelled
phoneme list --status tag_failed   # find recordings whose auto-tagging failed

# Limit the number of results returned (with optional offset for pagination)
phoneme list --limit 10
phoneme list --limit 10 --offset 20

# Filter by tag (numeric id or tag name)
phoneme list --tag work

# Full-Text Search via FTS5
phoneme list --search "rust migration"

# Semantic (embedding) search instead of an FTS5/list query — same engine as
# `phoneme search`, reusing --limit (default 20) as the result cap
phoneme list --semantic "database migration plan"

# Filter by recording type: all (default), single (voice notes), or meeting.
# Applied by the daemon in SQL, before --limit/--offset, so pages stay full.
phoneme list --kind meeting

# Run (or list) a saved search by id — the daemon parses the stored filter and
# runs the list query server-side, so a saved search is reproducible from the CLI.
phoneme list --saved          # list the saved searches (id + name)
phoneme list --saved ss_a1b2  # run the saved search with that id
```

### 👁️ `phoneme show <ID>`

Display the details of a single recording by its ID.

```bash
phoneme show 20260519T143500823

# Print only the audio path (useful for shell piping)
phoneme show 20260519T143500823 --audio-path-only

# Print the preserved ORIGINAL (machine) transcript, before AI cleanup
phoneme show 20260519T143500823 --original

# Print the unedited pipeline transcript (transcribed + cleaned, before your
# hand edits)
phoneme show 20260519T143500823 --unedited

# Print the machine transcript segments as a timeline: start-end offsets,
# speaker label (when diarized), and text per line. Empty for recordings
# transcribed before segment capture existed -- retranscribe to backfill.
phoneme show 20260519T143500823 --segments
```

### 🔁 `phoneme retranscribe <ID>` (alias: `phoneme replay`)

Re-transcribe a saved recording using your current model settings.

```bash
phoneme retranscribe 20260519T143500823

# Use a different transcription model for this run only
phoneme retranscribe 20260519T143500823 --model ggml-large-v3.bin

# Force hooks on / off for this run (overrides the configured behavior)
phoneme retranscribe 20260519T143500823 --run-hooks
phoneme retranscribe 20260519T143500823 --no-run-hooks

# Skip the LLM cleanup step for this run only (produces the raw transcript)
phoneme retranscribe 20260519T143500823 --no-post-process

# Re-run through a specific Playbook recipe (by id or name) instead of the
# default pipeline — the CLI face of the GUI ↻ Re-run "Recipe to run" picker.
phoneme retranscribe 20260519T143500823 --recipe meeting_notes
phoneme retranscribe 20260519T143500823 --recipe "Meeting notes"
```

> **`--recipe <ID|NAME>`:** re-run the recording through a chosen Playbook
> recipe, matching the GUI's **↻ Re-run** "Recipe to run" picker. The value is
> resolved against your configured recipes — by id first, then
> case-insensitively by name — and the resolved id is sent to the daemon. Omit
> it for the **`default`** pipeline. A value matching no recipe is an error that
> lists the available recipes (no silent fallback to default). The `--model`
> override still applies independently as a one-time transcription-model
> override.

### ✨ `phoneme cleanup <ID>`

Re-run only the LLM cleanup ("post-processing") step on a recording's stored
transcript, without re-transcribing the audio. The preserved original transcript
is always the input, so cleanup is idempotent. Overrides apply to this run only
and are never written to config; passing `--provider` also forces cleanup on.

| Flag | Effect |
|------|--------|
| `--provider <PROVIDER>` | Use this cleanup provider for this run (also forces cleanup on). |
| `--model <MODEL>` | Use this cleanup model for this run. |
| `--prompt <PROMPT>` | Use this cleanup prompt for this run. |
| `--api-url <URL>` | Point cleanup at this endpoint for this run. |
| `--api-key <KEY>` | Authenticate cleanup with this key for this run. |

> Passing a key via `--api-key` exposes it to any local process that can read
> the process table (`ps`, Task Manager, shell history). Prefer the
> `PHONEME_CLEANUP_API_KEY` environment variable — `--api-key` reads from it
> when the flag is omitted, and the env var stays out of the process table and
> shell history.

```bash
phoneme cleanup 20260519T143500823
phoneme cleanup 20260519T143500823 --provider ollama --model llama3.1
phoneme cleanup 20260519T143500823 --prompt "Fix grammar only"

# Point this run at a different OpenAI-compatible endpoint + credentials
phoneme cleanup 20260519T143500823 \
  --provider openai \
  --api-url https://api.example.com/v1 \
  --api-key sk-...
```

### 📝 `phoneme summarize <ID>`

Generate (or regenerate) an LLM summary of a recording's current transcript and
store it. `--model` / `--prompt` override the configured summary settings for
this run only.

```bash
phoneme summarize 20260519T143500823
phoneme summarize 20260519T143500823 --model llama3.1
phoneme summarize 20260519T143500823 --prompt "Three bullet points, no preamble."
```

### ✨ `phoneme suggest-tags <ID>`

Re-run the LLM tag-suggestion step on a recording on demand (the CLI face of the
GUI ✨ Suggest button), regardless of the `auto_tag.auto` gate. The command
awaits the model, then returns; the suggestions land on the recording. Review
them with `phoneme tag suggestions <ID>`. Errors when the recording has no
transcript yet (exit 6) or the id is unknown (exit 7).

```bash
phoneme suggest-tags 20260519T143500823
```

### ✏️ `phoneme edit <ID>`

Edit a recording's transcript and/or metadata. Any combination of the edits
below applies in one invocation:

- **Transcript** — `--text "…"`, or from stdin when no metadata flag and no
  `--text` is given.
- **Title** — `--title "…"` sets a user-owned title (the pipeline never
  overwrites it on a later retranscribe); `--clear-title` (or `--title ""`)
  reverts to auto-generation (the title empties now and regenerates on the next
  pipeline run).
- **Favorite** — `--favorite` / `--unfavorite` star or unstar the recording
  (the Favorites view).

```bash
# Transcript edit (the original behavior): --text or stdin
phoneme edit 20260519T143500823 --text "Corrected transcript."
echo "Corrected transcript." | phoneme edit 20260519T143500823

# Set or clear the display title
phoneme edit 20260519T143500823 --title "Q3 Planning Sync"
phoneme edit 20260519T143500823 --clear-title

# Star / unstar
phoneme edit 20260519T143500823 --favorite
phoneme edit 20260519T143500823 --unfavorite

# Combine: fix the text and set a title in one call
phoneme edit 20260519T143500823 --text "Fixed." --title "Standup notes"
```

A metadata-only edit (e.g. just `--favorite`) never blocks reading stdin.

### 🔁 `phoneme find-replace <ID> <FIND> <REPLACE>`

Find-and-replace **literal** text (not a regex) across a recording's stored
transcript. Case-insensitive by default; pass `--case-sensitive` for an exact
match. Only the live transcript is rewritten — the preserved original (machine)
and unedited (pipeline) copies stay intact, so the change is revertible, and the
timing layers are re-flowed onto the result like any hand edit. A no-match is a
no-op (nothing is written). Prints the number of occurrences replaced.

Pass **`--library`** to run the same literal replacement across **every**
recording in one shot — the positionals are then `FIND REPLACE` (no id).
Recordings with no match are left untouched (no version churn, no event), so
only the ones that actually change are rewritten. Prints how many occurrences
were replaced and across how many recordings.

```bash
# Fix a recurring misspelling across the whole transcript
phoneme find-replace 20260519T143500823 "teh" "the"

# Exact-case replacement
phoneme find-replace 20260519T143500823 "API" "api" --case-sensitive

# Machine-readable count
phoneme --json find-replace 20260519T143500823 "teh" "the"   # → {"replaced":3}

# Library-wide: fix a name everywhere at once
phoneme find-replace --library "Jon" "John"
phoneme --json find-replace --library "Jon" "John"
#   → {"recordings_changed":4,"total_replacements":11}
```

### ✂️ `phoneme clip <ID> <START> <END> [OUT]`

Export a time range of a recording's audio to a new WAV. `START` and `END` are
seconds as floats (e.g. `12.5`); the range is `[start, end)`, sliced on
sample-frame boundaries, and `END` is clamped to the recording's duration. The
clip is written in the source's audio format. When `OUT` is omitted the clip
lands next to the source recording with a `_clip_<start>-<end>` suffix. Prints
the path written.

```bash
# Cut 12.5s–30s into a sibling _clip_ file next to the source audio
phoneme clip 20260519T143500823 12.5 30

# Write to an explicit path
phoneme clip 20260519T143500823 12.5 30 highlight.wav
```

A non-finite or negative bound, `start >= end`, or two seconds that round to the
same millisecond are rejected locally (exit 1) before any daemon work. With
`--json`, prints `{"path": "<written-wav>"}`.

### 🗒️ `phoneme notes <ID>`

Get or set a recording's free-form notes (independent of the transcript).

```bash
# Print the current notes
phoneme notes 20260519T143500823

# Set the notes
phoneme notes 20260519T143500823 --set "Follow up with Alex."
```

### 🎭 `phoneme speaker`

Name and correct a recording's diarized speaker labels (the CLI face of the GUI
speaker chips and the in-recording speaker correction). Every `<LABEL>` is the
1-based `[Speaker N]` index from the transcript; `<IDX>` values are the 0-based
segment indices from `phoneme show --segments`.

**Naming** (`rename` / `clear`) never rewrites the transcript text — names are
applied at display/export time, so a rename is reversible:

```bash
# Give [Speaker 2] a display name
phoneme speaker rename 20260519T143500823 2 "Sarah"

# Clear a speaker label's custom name (revert to "Speaker N")
phoneme speaker clear 20260519T143500823 2
```

**Correcting assignments** (`reassign` / `merge` / `split`) actually changes
which segment belongs to which speaker. The stored `transcript_segments` stays
authoritative (the timeline / Synced views re-derive from it) and the prose
transcript's `[Speaker N]:` markers are rebuilt to match, in one transaction —
so the change shows up everywhere the user sees speakers:

```bash
# Reassign segment 5 to [Speaker 2] (a brand-new label simply starts existing)
phoneme speaker reassign 20260519T143500823 5 2

# Merge [Speaker 2] into [Speaker 1]: every 2-segment becomes 1, then 2 is gone.
# Speaker 1 keeps its name (adopts 2's only if 1 is unnamed); 2's captured
# voiceprint is dropped (the centroid is per-label — a retranscribe re-captures
# the merged label) and any affected named voice is recomputed.
phoneme speaker merge 20260519T143500823 2 1

# Split segments 4 and 7 off [Speaker 1] onto a fresh [Speaker 3]
# (the new label has no name or voiceprint until you name / re-enroll it)
phoneme speaker split 20260519T143500823 1 3 4 7
```

A label below 1, a self-merge, a split onto the same label, or a negative index
is rejected locally (exit 1) before any request is sent. An unknown segment
index, or a `merge from` / `split` index that doesn't currently carry the named
label, errors with no partial write.

### 🔎 `phoneme search <QUERY>`

Semantic (embedding) search over transcripts. Requires semantic search to be
enabled and the embedding model present. Prints `score  id  preview` per hit.

```bash
phoneme search "database migration plan"
phoneme search "database migration plan" --limit 5

# Scope a meaning-search like the Library: --tag (id or name), --status, --kind
# (single|meeting). The scope restricts the candidate set; an unscoped search is
# unchanged. Combinable.
phoneme search "budget" --tag work
phoneme search "budget" --status done --kind meeting
```

> `phoneme list --semantic "<query>"` runs the same search, reusing `--limit`
> and forwarding any `--tag` / `--status` / `--kind` scope.

**`--like <RECORDING_ID>`** — "more like this": instead of embedding a text
query, rank the library by similarity to a stored recording, using its
already-stored vectors. The source recording (and the other track of its own
meeting) never appears in the results. Works even when the embedding model
isn't loaded — only requires that the source recording is indexed; a
recording with no embeddings yet errors with a clear "isn't indexed yet"
message (re-embed or wait for the pipeline). `--like` and a text query are
mutually exclusive; `--limit` applies as usual.

```bash
phoneme search --like 20260519T143500823
phoneme search --like 20260519T143500823 --limit 5
```

### 💬 `phoneme ask "<QUESTION>"`

Ask a natural-language question answered **only** from your own transcripts —
local RAG with citations. The daemon embeds the question, retrieves the top
grounding chunks via the **same** hybrid (vector + FTS5/RRF) retriever as
`phoneme search`, builds a citation-instructed prompt, and streams the answer
through the configured `[llm_post_process]` provider. So it needs **both**
semantic search enabled (the embedding model loaded) **and** an LLM provider
configured; either missing exits `6` (invalid config). Nothing is persisted.

Output: the numbered **Sources** first (`[n] label (relevance%)`), then the
answer streamed to stdout — its inline `[n]` markers map back to those sources.
If nothing in your recordings matches, it says so and never invents an answer.

```bash
phoneme ask "what did we decide about the database migration?"
phoneme ask "summarize the open questions from my 1:1s" --top-k 12

# Scope the answer like the Library: --tag (id or name), --status, --kind
# (single|meeting). Combinable; an unscoped ask searches the whole library.
phoneme ask "what are my action items?" --tag work --kind meeting
```

- `--top-k <N>` — max grounding chunks to retrieve (default `8`, clamped
  server-side).
- `--json` — collect the whole stream into `{ "answer": "...", "sources": [...] }`
  instead of printing it live, where each source carries
  `{ n, recording_id, label, chunk_index, snippet, relevance }` so `[n]` resolves
  to `sources[n-1].recording_id`.

A provider failure mid-answer (e.g. the model truncated, or the endpoint went
away) prints to stderr and exits non-zero; the partial answer printed so far is
left intact.

### 🧬 `phoneme reembed`

Clear every stored embedding and re-embed the whole library with the
currently-configured embedding model. Run this after changing the embedding
model — a different model/dimension makes old vectors unsearchable. Returns
immediately; the re-embed runs in the background on the daemon (watch progress
in the daemon log).

```bash
phoneme reembed
```

### 🪝 `phoneme refire-hook <ID>`

Re-run the post-processing hook against a recording's already-stored transcript,
without re-transcribing. The hook runs in the background; observe the result via
`phoneme watch` (`hook_done` / `hook_failed` events). `--command` re-fires a
specific hook instead of the configured default — for safety the daemon only
accepts a command already present in the configured hook allowlist.

```bash
phoneme refire-hook 20260519T143500823
phoneme refire-hook 20260519T143500823 --command "python notify.py"
```

### 📜 `phoneme queue`

Inspect and manage the transcription pipeline queue. With no subcommand,
defaults to `list`.

```bash
# List the in-flight item plus everything still pending (table)
phoneme queue
phoneme queue list

# Inbox depth counts (pending / processing / done / failed)
phoneme queue counts

# Pause / resume the queue, or check whether it's paused
phoneme queue pause
phoneme queue resume
phoneme queue status

# Set the exact pending claim order (worker claims in this order)
phoneme queue reorder 20260519T143500823 20260519T143501999

# Remove one still-pending recording from the queue
phoneme queue cancel 20260519T143500823

# Cancel the item currently being processed (abort the in-flight work)
phoneme queue cancel-processing 20260519T143500823

# Skip the LLM step (cleanup / summary / tagging) currently running for the
# active item — the pipeline continues with whatever comes next. A no-op when
# no LLM stage is streaming (transcription and hooks aren't skippable; use
# cancel-processing for those). Mirrors the queue panel's ⏭ button.
phoneme queue skip

# Remove ALL still-pending items at once
phoneme queue cancel-all

# Empty the inbox failed/ quarantine ("dismiss failed")
phoneme queue clear-failed

# Dismiss ONE quarantined item by id (the per-item counterpart to clear-failed)
phoneme queue dismiss-failed 20260519T143500823
```

### 🗑️ `phoneme delete <ID>`

Delete a recording and its associated audio file.

```bash
phoneme delete 20260519T143500823

# Keep the original .wav file on disk, just remove the catalog entry
phoneme delete 20260519T143500823 --keep-audio
```

### 🪝 `phoneme hook`

Test and manage your post-processing hooks.

```bash
# Run the configured hook with a mock payload to test your script
phoneme hook test
```

### 🔄 `phoneme export`

Bulk export all audio and metadata into a zip archive, or export a recording's
transcript segments as a caption file (SRT or WebVTT).

**Library zip export**

```bash
phoneme export backup.zip
```

**Caption export flags**

| Flag | Description |
|------|-------------|
| `--captions <RECORDING_ID>` | Export captions for this recording instead of zipping the library. |
| `--format <srt\|vtt>` | Caption format: `srt` (default) or `vtt`. |
| `-o`, `--out <FILE>` | Write captions to FILE. Use `-` for stdout. Defaults to `<recording-id>.srt` / `<recording-id>.vtt` in the current directory. |

**Examples**

```bash
# Export captions as SRT (default) for a recording — writes 20260519T143500823.srt
phoneme export --captions 20260519T143500823

# Export as WebVTT to an explicit path
phoneme export --captions 20260519T143500823 --format vtt -o captions/meeting.vtt

# Pipe SRT directly to another tool
phoneme export --captions 20260519T143500823 -o -
```

Recordings that have no stored segments (e.g. transcribed before timing data
was captured) print a clear message and exit non-zero — retranscribe the
recording to generate segments first.

### ♻️ `phoneme import-backup`

Restore a library backup zip — the inverse of `phoneme export <FILE>`. Each
recording in the archive is re-inserted into the catalog and its audio copied
into the configured audio directory.

```bash
phoneme import-backup backup.zip
```

The daemon holds the catalog database open while it runs, so `import-backup`
shuts a running daemon down first and waits for it to release the file (like
`doctor --rebuild-catalog`). Start the daemon again afterwards with
`phoneme daemon start`.

Restore is **idempotent**: a recording whose id already exists in the catalog
is skipped (counted, never overwritten), so re-running on the same backup never
duplicates a row or reverts a hand edit you made since. The command prints how
many recordings it imported and how many it skipped. What round-trips is what
the export captured — the recording metadata, transcript, and tags, plus the
audio; derived data (segments, embeddings, voiceprints) is regenerated by a
retranscribe.

### 🏷️ `phoneme tag`

Manage recording tags. Wherever a `<TAG>` is taken (attach / detach / merge), it
accepts either a numeric tag id or a tag name.

```bash
# List tags attached to a recording; --all also includes orphaned (unused) tags
phoneme tag list
phoneme tag list --all

# Add a new tag with an optional color
phoneme tag add work --color "#ff0000"

# Rename and/or recolor an existing tag (by id)
phoneme tag update 1 work --color "#4caf50"

# Delete a tag by ID
phoneme tag delete 1

# Attach / detach a tag (by name or id) to a recording
phoneme tag attach 20260519T143500823 work
phoneme tag detach 20260519T143500823 work

# List the tags attached to one recording
phoneme tag for 20260519T143500823

# Show how many recordings each tag is attached to
phoneme tag usage

# Review one recording's pending auto-tag suggestions
phoneme tag suggestions 20260519T143500823

# Approve a suggestion (creates + attaches the real tag, drops the proposal)
phoneme tag suggestions 20260519T143500823 --approve work

# Dismiss a suggestion (drops the proposal, attaches nothing)
phoneme tag suggestions 20260519T143500823 --dismiss spam

# Drop every pending auto-tag suggestion across the whole library (approved
# tags stay attached; only not-yet-decided proposals are discarded)
phoneme tag clear-suggestions

# Merge one tag into another: re-point all recordings, then delete the source
phoneme tag merge old-name work
```

### 🎭 `phoneme profile`

Manage config profiles (named full-config snapshots).

```bash
# List saved profiles
phoneme profile list

# Save the current config as a named profile snapshot
phoneme profile save work_mode

# Switch the active config to a saved profile and reload the daemon
phoneme profile use work_mode
```

`save` and `list` are purely local (they copy/read files under
`%APPDATA%\phoneme\profiles\`); `use` overwrites the live `config.toml` with
the snapshot and sends the daemon a reload. The GUI equivalent is
**Settings → Managers → Profiles**.

### 🩺 `phoneme doctor`

Run a health check on your system.

Checks: config file presence, audio-directory writability, free disk space on
the volumes holding the recordings and the app data (catalog/queue/models),
hook command resolvability, model-file integrity (the Whisper model — plus the
live-preview, semantic-search and diarization models when those features are
on: missing, 0-byte and implausibly small files are all caught), Whisper
server reachability, the dedicated live-preview server (when configured on its
own port), and Ollama (optional).

Every check carries a category describing how severe a failure is:

- **critical** — recording or transcription is broken (unwritable audio dir,
  missing/corrupt Whisper model, unreachable Whisper server, under ~500 MB of
  free disk);
- **warning** — something is degraded but capture + transcription still work
  (hook not resolvable, optional model missing, under ~2 GB of free disk);
- **info** — informational only; never fails the run.

Passing checks print as one line. Failing checks get a colored category badge
plus two indented lines: what the check verifies, and a `fix:` hint with the
next step. The exit code is non-zero when any warning- or critical-category
check fails.

```bash
phoneme doctor

# Attempt repairs for failed checks: when the Whisper / live-preview server
# probe fails, asks the daemon to sweep hung/orphaned whisper-server processes
# and respawn them from config, then re-probes and reports the fresh results.
phoneme doctor --fix

# Force the catalog to rebuild itself from orphan files on disk. Asks a
# running daemon to shut down and WAITS (up to 15s) for it to actually exit
# before deleting catalog.db (plus its -wal/-shm sidecars) — if the daemon
# won't die in time, the command fails and leaves the catalog untouched.
phoneme doctor --rebuild-catalog

# NON-destructive recovery: ask the running daemon to scan the audio folder and
# re-link any .wav files that have no catalog row (re-importing + re-transcribing
# them), leaving every existing recording untouched. Prefer this over
# --rebuild-catalog when you've just lost rows, not the whole catalog.
phoneme doctor --reimport
```

With `--json`, each check object keeps the original `name`/`ok`/`detail` keys
and additionally carries `category` (`"critical" | "warning" | "info"`),
`explanation`, and `fix_hint` (string or null) — additive only, so existing
consumers keep working.

### ⚙️ `phoneme config`

Manage configuration.

```bash
# With no subcommand: print the active config as TOML. Secret values (API keys,
# the webhook HMAC secret) are masked as <redacted> so the dump is safe to paste
# or pipe.
phoneme config

# Print the real secret values instead of <redacted> — pass it only when you
# deliberately need the keys.
phoneme config --show-secrets

# Print the path to the active config file
phoneme config path

# Set a config value (parses bool/int/float, else string)
phoneme config set whisper.mode external

# Hot-reload the configuration file from disk. The daemon immediately applies
# changes (hotkeys, models, …) without restarting.
phoneme config reload
```

`config set` semantics:

- **It writes the file the daemon actually reads** — the `PHONEME_CONFIG`
  override when that env var is set, otherwise the per-user default
  (`config path` prints the default; the override wins everywhere).
- **The full updated config is validated first.** A value with the wrong type
  for its field, or one that fails the same `validate()` the daemon runs on
  load (e.g. an out-of-range `recording.sample_rate`), is rejected with exit
  code for invalid config and **nothing is written** — `config set` can never
  produce a file the daemon refuses to load.
- **The write is atomic**: the new content lands in a `.toml.tmp` sibling and
  is renamed over the real file, so a crash mid-write leaves the previous
  config intact rather than a truncated half-file.

> The config is **validated automatically** when the daemon loads or reloads it; an invalid file is rejected with an error. There is no separate `config validate` subcommand.

### 📡 `phoneme watch`

Subscribe to live daemon events as a stream of JSON objects. Useful for building your own UI or integration on top of Phoneme.

```bash
phoneme watch
```

### 👻 `phoneme daemon`

Send daemon control commands.

```bash
# Spawn the daemon in a detached background process
phoneme daemon start

# Print the daemon's status
phoneme daemon status

# Graceful shutdown: sends the Shutdown IPC and waits (up to ~5s) for the
# daemon to actually exit
phoneme daemon stop
```

`daemon stop` is the full shutdown chain: the daemon acknowledges the request
**before** exiting, stops and queues any in-flight recording (nothing is
corrupted mid-write; the next daemon run transcribes it), kills the
whisper-server(s) it spawned, and stops an Ollama it auto-launched — an Ollama
you started yourself is never touched. Stopping an already-stopped daemon
prints `daemon is not running` and succeeds (it never spawns one just to stop
it).

### 🏷️ `phoneme version`

Print version and commit info.

```bash
phoneme version
```

### ⌨️ `phoneme completions <SHELL>`

Print a shell-completion script for the chosen shell to stdout. This is pure
local generation — it never contacts the daemon, so it works before the daemon
is even running. Supported shells: `bash`, `zsh`, `fish`, `powershell`,
`elvish`.

Install one-liners:

```bash
# bash — drop the script where bash-completion looks for it
phoneme completions bash > ~/.local/share/bash-completion/completions/phoneme

# zsh — write into a directory on your $fpath (here a personal completions dir),
# then ensure that dir is on fpath and compinit runs in ~/.zshrc
mkdir -p ~/.zfunc
phoneme completions zsh > ~/.zfunc/_phoneme
# in ~/.zshrc, before `compinit`: fpath=(~/.zfunc $fpath)

# fish
phoneme completions fish > ~/.config/fish/completions/phoneme.fish
```

```powershell
# PowerShell — load completions for the current session only
phoneme completions powershell | Out-String | Invoke-Expression

# Persist across sessions: append the same line to your profile
'phoneme completions powershell | Out-String | Invoke-Expression' |
  Add-Content $PROFILE
```

## 🧠 Daemon Management

While the daemon is usually auto-spawned by the CLI, the System Tray application, or `phoneme daemon start`, you can run it directly:

```powershell
# Run the daemon in the foreground
phoneme-daemon

# Run with explicit debug logging (PowerShell)
$env:RUST_LOG = "debug"; phoneme-daemon
```
