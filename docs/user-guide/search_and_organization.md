# 🔍 Search & Organization

Phoneme is designed to be your second brain for spoken thought. If you record everything, you need to be able to find everything.

## ⚡ Full-Text Search (FTS5)

At the top of the main UI, you will find a global Search bar.

![Main recordings view](../screenshots/main.png)

Under the hood, Phoneme uses a lightning-fast SQLite FTS5 (Full-Text Search) engine. Every time you transcribe a recording, the text is instantly indexed.

This means:
- **Instant Results**: Searching through 5,000 recordings takes less than 10 milliseconds.
- **Prefix Matching**: If you search for "phono", it will match "phoneme", "phonology", etc.
- **Compound Queries**: You can type multiple words like `"marketing budget"` to find recordings that contain both concepts.
- **Model Names**: The search box also matches the **models** behind each recording. Type a model name (e.g. `medium`, `gpt-4o-mini`, `llama3.2`) to find every recording that model transcribed, cleaned, summarised, titled, tagged, or diarized.

## 🧠 Semantic search (meaning-based)

Keyword search only matches words you actually said. **Semantic search** uses offline ONNX embeddings to find recordings by *concept* — paraphrases, related ideas, vague memories. Transcripts are embedded per sentence-aware chunk, and the semantic ranking is fused with the FTS5 keyword ranking (RRF) so a query finds the recording whether you remember the gist or the one distinctive word. Each hit shows a calibrated relevance chip.

Enable it from the **Semantic Search** settings section (search Settings for "Semantic", or set `semantic_search.enabled = true`). Full guide: [Semantic Search](semantic_search.md).

Once a recording is indexed you can also search *from* it: the **✨ Similar** button in its detail view fills the list with the semantically closest recordings — no query to type. See ["More like this"](semantic_search.md#more-like-this).

## 🏷️ Organizing with Tags

Tags are the primary way to organize your recordings into projects, categories, or states.

### ➕ Creating and Applying Tags

1. Select one or more recordings from the list (you can `Shift+Click` or `Ctrl+Click` to select multiple).
2. The Action Bar will appear at the top.
3. Click the **Tag** icon.
4. Type a new tag name and press `Enter` to create it, or select an existing tag.

### 🎨 The Tag Manager

You can manage your entire tag taxonomy from the **Settings → Tags** tab (see [Settings Overview](settings_overview.md)).

Here you can:
- **Rename** tags (this will update all associated recordings instantly).
- **Recolor** tags (Phoneme provides a beautiful palette of highly legible colors).
- **Delete** tags (this removes the tag from all recordings, but *does not* delete the recordings themselves).

### 🪄 Inline tag chips

In a recording's detail view, tags appear as colored chips. Click a chip to edit its name and color inline, or add a new tag right there — no need to open the Tag Manager for quick edits.

## ⭐ Favorites

Click the **star column** (the leading ★ in the recordings list) to mark a
recording as a favorite. The Library sidebar's **Favorites** filter shows only
starred recordings, alongside All / Voice Notes / Meetings. Stars are stored in
the catalog, so they survive restarts and travel with exports.

## 🎙️ The Source column

The recordings list's **Source** column (shown by default; toggle it under
**Settings → Appearance → Visible Columns**) tells you what each recording was
**actually captured from**:

| Icon | Source | What it means |
|------|--------|---------------|
| 🎤 | **Microphone** | Captured from your microphone. |
| 🔊 | **System audio** | Captured from the system-audio loopback (what your speakers were playing) — Windows. |

The column reads each recording's **real** capture source, not the setting that
was in effect when you hit record. Single voice notes and the individual tracks
of a meeting alike report their own source, so a binding that records the mic and
one that records system audio are easy to tell apart at a glance. (Older
recordings made before this was tracked have no stored source and fall back to
**Microphone**.)

> [!TIP]
> If you hide the Source column, the source still shows as a small 🎤 / 🔊 icon
> in front of the transcript preview, so meeting tracks never lose it. Choose
> what each hotkey records — mic vs system audio — under
> [Hotkeys & recording modes](hotkeys_and_recording_modes.md); meetings always
> record **both** tracks (see [Meeting Mode](meeting_mode.md)).

## 🎭 Named speakers

When a recording is [diarized](diarization_and_whisper.md), its transcript reads
as `[Speaker 1]: …` / `[Speaker 2]: …`. You can rename any `Speaker N` to a real
name — **Sarah**, **Alex**, **You** — and the name sticks to that recording.

| Where | How |
|-------|-----|
| **Detail / merged view** | Click a speaker chip and type a name. Renaming there updates the speaker everywhere in that recording at once. |
| **CLI** | `phoneme speaker rename <id> <label> "<name>"` — `<label>` is the 1-based `[Speaker N]` index. Clear a name with `phoneme speaker clear <id> <label>`. |

The name is stored per recording and applied everywhere that speaker appears —
the **transcript**, the **timeline**, the **synced** view, and the **merged**
meeting conversation — so one rename relabels them all.

> [!NOTE]
> Names are applied at display and export time; the stored transcript keeps its
> canonical `[Speaker N]` markers. A rename is therefore reversible — clear it
> and the label reverts to `Speaker N` — and you can re-rename a speaker as often
> as you like.

## 🔖 Saved searches

A saved search snapshots **everything** the library is filtered by — search
text, the semantic toggle, library type (including Favorites), tag, status,
date range, and sort order — and restores it all in one click.

- **Quick popup:** the 🔖 button in the header saves the current filters under
  a name and re-applies saved ones.
- **Full manager:** **Settings → Managers → Saved searches** (or `g` then `S`)
  lists every saved search with its full description, and can apply, rename,
  **update to the current filters**, or delete each one.

## ◫ Side-by-side

Multi-select exactly **two** recordings and press **`\`** (or use the bulk
bar's *Side by side* button) to open both transcripts next to each other in
full editors — vim keys and `:w` work per pane. Great for comparing takes or
cross-referencing two meetings.

## 🔍 List zoom

`Ctrl + =` / `Ctrl + -` zoom the recordings list bigger/smaller (`Ctrl + 0`
resets; `Ctrl + scroll` over the list works too). The zoom level is remembered
per device.

## 🔎 Filtering Views

You can drill down into your catalog using the Filter pills above the recordings list.

- **Library filter**: switch between **All**, **Voice Notes** (single recordings), **Meetings** (multi-track meeting recordings), and **Favorites** (starred). This mirrors the CLI `phoneme list --kind all|single|meeting`. The filter is applied by the daemon before pagination, so every page is full of the chosen kind — including Favorites pages deep into a large library.
- **Status filter**: the header's status dropdown narrows the list to one processing state. The full set is below.
- **Tag Filters**: Click "Filter by Tag" to only show recordings that have specific tags attached. You can select multiple tags to narrow your view.
- **Date Filters**: Click the Date pill to restrict your view to "Today", "This Week", or select a custom date range from the calendar.

### 🚦 Recording statuses

Every recording carries exactly one status — its place in the pipeline, or where
it came to rest. The status dropdown filters by any one of these, and each value
is searchable. Statuses fall into three groups: **in-flight** (the pipeline is
still working), **at rest** (Done), and **terminal failures / cancellation**.

| Status | Group | What it means |
|--------|-------|---------------|
| **Recording** | In-flight | Live capture is happening right now — audio is still being added. |
| **Queued** | In-flight | Claimed for processing but waiting in the queue; the worker hasn't started it yet. Distinct from **Transcribing** so you can tell waiting from working. |
| **Paused** | In-flight | Capture is paused — no audio is being added until you resume. |
| **Transcribing** | In-flight | Whisper (or your cloud provider) is producing the transcript. |
| **Cleaning Up** | In-flight | The optional AI cleanup step is rewriting the transcript. |
| **Summarizing** | In-flight | The optional auto-summary step is running. |
| **Tagging** | In-flight | The optional auto-tag step is suggesting tags. |
| **Hook Running** | In-flight | Your post-transcription hook (or webhook) is running. |
| **Done** | At rest | Fully processed — the terminal success state. |
| **Cancelled** | Terminal | You cancelled the run yourself (a queued item removed, or an in-flight transcription aborted). Terminal like the failures, but nothing broke — it is **never** counted or surfaced as a failure. |

> [!NOTE]
> A recording's status is independent of its [Library kind](#-filtering-views) and
> tags — you can combine a status filter with a tag, a date range, and Voice
> Notes / Meetings / Favorites at the same time.

#### Failure states

Failures split into two kinds, and only the terminal ones leave you without a
usable transcript:

| Status | Kind | What failed |
|--------|------|-------------|
| **Transcription Failed** | Terminal | Transcription itself didn't produce a transcript. The core step failed — re-transcribe to retry. |
| **Hook Failed** | Terminal | The post-transcription hook (or webhook) returned an error. |
| **Cleanup Failed** | Optional-step | The transcript is intact and usable — only the AI cleanup didn't land. |
| **Summary Failed** | Optional-step | The transcript is intact — only the auto-summary step failed. |
| **Title Failed** | Optional-step | The transcript is intact — only the auto-title step failed. |
| **Tagging Failed** | Optional-step | The transcript is intact — only the auto-tag step failed. |

> [!IMPORTANT]
> **Optional-step failures don't mean a broken recording.** When cleanup,
> summary, title, or tagging fails, the transcript is complete and searchable —
> only that one enrichment is missing. These statuses are filterable and
> searchable so you can find them and re-run just the failed step from the
> [Re-run menu](#-the-re-run-menu), instead of having a failure quietly swallowed.
> Only **Transcription Failed** and **Hook Failed** are true terminal failures.

## 🔁 The Re-run menu

Each recording has a **Re-run** menu for reprocessing without re-recording: **Re-transcribe** (optionally a different model, optionally skip cleanup), **Re-run cleanup** (one-off provider/model/prompt), **Regenerate summary**, and **Re-fire hook**. Overrides apply to that single run and are never saved to config. See [Smart Cleanup](smart_cleanup.md) and [Providers & Models](providers_and_models.md#one-time-overrides-re-run-menu).

## 📦 Bulk actions

Select multiple rows with **Shift+Click** or **Ctrl+A**. The action bar supports batch **delete**, **re-transcribe**, and **export**. A bulk delete asks for the delete mode once and applies it to every selected recording.

## 🗑️ Deleting recordings

Delete from the action row under an open recording, the bulk action bar, the `Delete` key, or `dd` (vim navigation). The confirmation offers two modes:

- **Delete everything** (default) — removes the recording and its audio file from disk.
- **Keep the audio file** — removes the entry from your library (transcript, notes, tags) but leaves the audio file on disk. Same as the CLI's `phoneme delete --keep-audio`.

Either way the rows disappear immediately with an **Undo** toast. Nothing is actually deleted until the toast runs out, so Undo always brings everything back. Check **Don't ask again** to skip the dialog from then on — future deletes reuse the mode you had selected when you checked it.

## 📄 Pagination

Large catalogs load in pages with **Load more** at the bottom (infinite scroll). This keeps the UI responsive with thousands of recordings.
