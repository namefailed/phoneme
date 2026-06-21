# ✅ Tasks from Voice

Phoneme can pull **task-shaped action items** out of a transcript — the to-dos
you committed to or asked for ("send the roadmap by Friday", "book the room") —
into a checkable list you own. It's an AI enrichment step, like Auto-Tagging, but
the result is a **list you tick off** rather than chips you approve.

## The Tasks list (detail pane)

Open any recording and look for the **✅ Tasks** section under the tags:

- Each action item is a row with a **checkbox**. Tick it to mark the task done —
  done tasks dim and strike through, and sort below the open ones.
- A deadline the speaker mentioned shows as a muted suffix, e.g.
  *Send the roadmap **(by Friday)***. This is the model's phrase stored
  **verbatim** — Phoneme does not turn it into a calendar date or a reminder.
- The **✅ Extract** button runs the task-extraction step on this recording right
  now (even if your pipeline doesn't run it automatically). Re-running **replaces**
  the list with a fresh extraction — but **any task you already checked off stays
  checked** as long as its text comes back the same. A flaky run that finds
  nothing leaves your existing list untouched.

## Browsing tasks across the library (sidebar)

The left sidebar has a **Tasks** section with two filter rows:

- **Open** — recordings that still have at least one unfinished task.
- **All tasks** — recordings with any extracted task.

Each shows a count. Click a row to filter the library to those recordings; click
it again to clear.

## Turning it on

Task extraction is **opt-in** — it doesn't run on every recording by default.
There are three ways to use it:

1. **On demand** — the **✅ Extract** button on any recording (above).
2. **Per pipeline** — add the built-in **Extract tasks** entry to a recipe in
   **Settings → 🎭 Playbook**, so every recording that runs that recipe gets a
   task list automatically.
3. **From the CLI** — `phoneme suggest-tasks <recording-id>`.

The **Extract tasks** Playbook entry is editable like any other: change its
provider, model, or prompt in the Playbook manager. With **Local Ollama** the
transcript never leaves your machine, exactly like Smart Cleanup and Auto-Tagging.

## From the CLI

```bash
# Pull tasks out of one recording (awaits the model)
phoneme suggest-tasks 20260519T143500823

# List every task across the library (open first)
phoneme tasks

# Only the still-open tasks
phoneme tasks --open

# Mark task #3 of a recording done (or undone) — the id is shown in the list
phoneme tasks done 20260519T143500823 3
phoneme tasks undone 20260519T143500823 3
```

## Notes & limits

- **`due_hint` is a phrase, not a date.** Phoneme stores the model's wording and
  shows it; it does **not** schedule reminders or sync to a todo app (that's on
  the roadmap).
- **Re-wording loses the checkbox.** Tasks are matched by their text, so if a
  re-extraction rephrases a task you'd completed, it reappears unchecked — the
  same text-matching limitation entities have.
- Deleting a recording deletes its tasks with it.
