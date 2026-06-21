# 🧭 Topic Timelines (Auto-Chapters)

A long recording is hard to skim as one wall of text. Phoneme can split it into
**chapters** — a titled span per topic, each with an optional one-line summary and
a real start/end time — so you can jump straight to the part you want. It's an AI
enrichment step, like Auto-Tagging or Tasks, but the result is a **navigable
timeline** rather than chips or a checklist.

## The Chapters view (detail pane)

Open a recording and switch to the **Chapters** view:

- Each chapter is a row: a **timestamp** (mm:ss), a **title**, and — when the
  model wrote one — a short **summary** line. Click a chapter to jump the
  transcript / player to that moment.
- The **✨ Generate** button runs the chapter step on this recording right now,
  even if your pipeline doesn't run it automatically. Re-generating **replaces**
  the chapter list with a fresh pass.
- Every boundary is **snapped to a real segment start**, so a chapter always lines
  up with where the audio actually changes — you never land mid-word.

A recording with no timing yet (still transcribing, or a provider that returned no
segments) simply has no chapters — that's a normal empty state, not an error.

## Turning it on

Chapter extraction is **opt-in** — it doesn't run on every recording by default.
Three ways to use it:

1. **On demand** — the **✨ Generate** button in the Chapters view (above).
2. **Per pipeline** — add the built-in **Chapters** entry to a recipe in
   **Settings → 🎭 Playbook**, so every recording that runs that recipe is
   chaptered automatically.
3. **From the CLI** — `phoneme chapters <recording-id>`.

The **Chapters** Playbook entry is editable like any other enrichment: change its
provider, model, or prompt in the Playbook manager. With **Local Ollama** the
transcript never leaves your machine, exactly like Smart Cleanup and Auto-Tagging.

## From the CLI

```bash
# Generate chapters for one recording and print them (awaits the model)
phoneme chapters 20260519T143500823

# Just print the already-stored chapters, without re-generating
phoneme chapters 20260519T143500823 --show
```

Each line prints as `mm:ss  Title`. Pass `--json` for machine-readable output.

## Notes & limits

- **Chapters describe the transcript at generation time.** If you make a large
  edit to the transcript afterward, the existing chapters stay as they were until
  you re-generate — they don't auto-rewrite (the same on-demand model the summary,
  tags, and tasks follow).
- **Boundaries snap to segments.** Chapter times are pinned to real segment
  starts, so they always align with the audio rather than to an arbitrary
  character offset.
- Deleting a recording deletes its chapters with it; chapters travel with library
  exports.
