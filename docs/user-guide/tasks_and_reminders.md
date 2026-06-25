# ✅ Tasks from Voice

Phoneme can pull **task-shaped action items** out of a transcript — the to-dos
you committed to or asked for ("send the roadmap by Friday", "book the room") —
into a checkable list you own. It's an AI enrichment step, like Auto-Tagging, but
the result is a **list you tick off** rather than chips you approve.

## The Tasks list (detail pane)

Open any recording and look for the **✅ Tasks** section inside the **Insights**
card below the transcript (it shares the card with [Entities](entities.md)). The
whole card collapses if you don't use it, and you can hide it entirely from
**Settings → Interface → Detail pane → Insights card**:

- Each action item is a row with a **checkbox**. Tick it to mark the task done —
  done tasks dim and strike through, and sort below the open ones.
- A deadline the speaker mentioned shows as a muted suffix, e.g.
  *Send the roadmap **(by Friday)***. This is the model's phrase stored
  **verbatim** — Phoneme does not turn it into a calendar date or a reminder.
- The **✅ Extract** button runs the task-extraction step on this recording right
  now (even if your pipeline doesn't run it automatically). Re-running **replaces**
  the AI's list with a fresh extraction — but **any task you already checked off
  stays checked** as long as its text comes back the same. A flaky run that finds
  nothing leaves your existing list untouched.

It's a list you edit, not just a result you read:

- **+ Add** a task by hand (the input row at the bottom of the section).
- **Edit** a task's text inline — double-click it, or use the ✎ button.
- **Delete** a task with the ✕ button.
- **Reorder** open tasks by dragging the ⠿ grip.
- **Hide done** with the toggle when you only want what's left.

Anything you **add or edit by hand is yours** — it's marked *manual* and **survives
a re-extraction**, where the AI only ever replaces its own suggestions.

## Browsing tasks across the library (sidebar)

The left sidebar has a **Tasks** section with two filter rows:

- **Open** — recordings that still have at least one unfinished task.
- **All tasks** — recordings with any extracted task.

Each shows a count. Click a row to filter the library to those recordings; click
it again to clear.

Below them, **📋 View all…** opens the **All tasks** modal — the flat,
cross-recording "everything I have to do" list. Every task in one place: tick it
off in line, filter by text, toggle **Open only**, and click a task's recording
pill to jump straight to it.

> Don't use this section? Hide it from the sidebar via **Settings → Appearance →
> Sidebar sections** — your tasks stay extracted; only the sidebar section is hidden.

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

# Mark a task done (or undone). The numeric arg is the task's DB row id — the
# `#N` shown beside it by `phoneme tasks` / `phoneme show`, NOT a per-recording
# ordinal.
phoneme tasks done 20260519T143500823 142
phoneme tasks undone 20260519T143500823 142

# Add a task by hand (kept across re-extraction); --due is optional
phoneme tasks add 20260519T143500823 "Send the roadmap" --due "by Friday"

# Edit a task's text (the due hint is kept unless you change it)
phoneme tasks edit 20260519T143500823 142 "Send the v2 roadmap"
phoneme tasks edit 20260519T143500823 142 "Send it" --due "Monday"   # change due
phoneme tasks edit 20260519T143500823 142 "Send it" --clear-due      # drop due

# Delete a task, or set the task order (row ids in the order you want them)
phoneme tasks delete 20260519T143500823 142
phoneme tasks reorder 20260519T143500823 145 142 144 141
```

Every task action mirrors the GUI exactly — the CLI, the detail-pane list, and
the daemon all go through the same IPC, so a task you add from the terminal shows
up in the app live, and vice-versa.

## Notes & limits

- **`due_hint` is a phrase, not a date.** Phoneme stores the model's wording and
  shows it; it does **not** schedule reminders or sync to a todo app (that's on
  the roadmap).
- **Done-state follows the text.** A completed AI task stays checked across a
  re-extraction as long as the wording comes back the same (minor punctuation /
  spacing differences are ignored). A substantial rephrase is treated as a new
  task and reappears unchecked — to make a task permanent, add or edit it by hand
  so it's *manual* and exempt from re-extraction entirely.
- Deleting a recording deletes its tasks with it.

## See also

Tasks are one of Phoneme's per-recording AI enrichments. The others work the same
way — opt-in, on-demand or per-pipeline, with hand edits that survive
re-extraction:

- [Entities](entities.md) — the people, orgs, topics, and terms a recording
  mentions, with a cross-recording browse facet.
- [Topic Timelines (Auto-Chapters)](topic_timelines.md) — a navigable,
  time-coded chapter list per recording.
