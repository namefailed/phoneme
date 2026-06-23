# 🔎 Entities

Phoneme can pull the **named things** out of a transcript — the people, companies,
topics, and special terms it mentions — into a typed, structured list. It's an AI
enrichment step, like Auto-Tagging or [Tasks](tasks_and_reminders.md), but the
result is a set of **classified entities** you can browse the whole library by.

Every entity has a **kind**:

| Kind | Icon | What it is |
|------|------|------------|
| **Person** | 👤 | A named individual ("Sarah Chen", "Dr. Patel"). |
| **Org** | 🏢 | A company, team, or institution ("Acme Corp", "the design team"). |
| **Topic** | 💡 | A subject or theme the recording is about ("Q3 roadmap", "onboarding"). |
| **Term** | 🔤 | A domain term, product name, or jargon worth tracking ("RRF", "speakrs"). |

## The Entities list (detail pane)

Open any recording and look for the **🔎 Entities** section under the transcript:

- Entities show as **chips grouped by kind** (People, then Organizations, then
  Topics, then Terms), each with its kind's icon.
- The **🔎 Extract** button runs the entity-extraction step on this recording
  right now (even if your pipeline doesn't run it automatically). Re-running
  **replaces** the AI's set with a fresh extraction — but **any entity you added
  or edited by hand stays** (it's marked *manual*; the AI only ever replaces its
  own).

It's a list you edit, not just a result you read:

- **+ Add** an entity by hand — pick a kind, type the value, Enter. (Adding one
  marks it *manual*.)
- **Edit** an entity's value inline — double-click the chip, type, Enter.
- **Delete** an entity with the **✕** on its chip.
- **Manage** — opens the library-wide [Entity manager](#cross-library-manager)
  for renaming and merging across every recording at once.

Anything you **add or edit by hand is yours** — it's *manual* and **survives a
re-extraction**, where the AI only ever replaces its own suggestions.

## Browsing the library by entity (sidebar)

The left sidebar has an **Entities** section: the **cross-recording entity facet**
— every distinct entity across your whole library, grouped by kind, each with a
count of how many recordings mention it.

Click an entity row to filter the library to the recordings that mention it; click
it again to clear. It's the entity counterpart of the tag list — "show me every
recording that mentions Acme Corp" without typing a search.

> Don't use this section? Hide it from the sidebar via **Settings → Appearance →
> Sidebar sections** — your entities stay extracted and searchable; only the
> sidebar facet is hidden.

## Turning it on

Entity extraction is **opt-in** — it doesn't run on every recording by default.
Three ways to use it:

1. **On demand** — the **🔎 Extract** button on any recording (above).
2. **Per pipeline** — add the built-in **Extract entities** entry to a recipe in
   **Settings → 🎭 Playbook**, so every recording that runs that recipe gets an
   entity set automatically.
3. **From the CLI** — `phoneme suggest-entities <recording-id>`.

The **Extract entities** Playbook entry is editable like any other enrichment:
change its provider, model, or prompt in the Playbook manager. With **Local
Ollama** the transcript never leaves your machine, exactly like Smart Cleanup and
Auto-Tagging.

## Cross-library manager

The detail pane's Entities **Manage** button opens the **Entity manager** — a
library-wide modal for curating extracted entities:

- **Rename** an entity's value in place, across every recording at once.
- **Merge** variants into one canonical value — fold "ACME", "acme corp", and
  "Acme" into **Acme Corp** everywhere.

Both operations mark the result *manual*, so the merged/renamed value **survives a
later re-extraction**. (A rename is a one-source merge under the hood.) Esc, an
overlay click, or the ✕ close the modal.

## From the CLI

```bash
# Pull entities out of one recording (awaits the model)
phoneme suggest-entities 20260519T143500823

# List the cross-recording entity facet (every kind, grouped, with counts)
phoneme entities

# Only one kind: person / org / topic / term
phoneme entities --kind person

# Add an entity by hand (kept across re-extraction): <id> <kind> <value>
phoneme entities add 20260519T143500823 person "Sarah Chen"

# Edit one entity, keyed by its current kind + value. Change the value and/or kind.
phoneme entities edit 20260519T143500823 org "acme" --to-value "Acme Corp"
phoneme entities edit 20260519T143500823 topic "roadmap" --to-kind term

# Delete one entity, keyed by its kind + value
phoneme entities delete 20260519T143500823 topic "roadmap"

# Library-wide merge: fold one or more variants into a canonical value (within a kind)
phoneme entities merge org "Acme Corp" acme ACME "acme corp"
```

`phoneme entities` (with no sub-action) is the facet — the CLI face of the
sidebar's browse-by-entity surface. To then *list the recordings* for one entity,
pass its value to `phoneme list`.

Every entity action mirrors the GUI exactly — the CLI, the detail-pane chips, and
the daemon all go through the same IPC, so an entity you add from the terminal
shows up in the app live, and vice-versa.

## Notes & limits

- **Manual entities are exempt from re-extraction.** Anything you add, edit, or
  merge is marked *manual* — the AI replaces only its own suggestions, never
  yours.
- **An unknown kind is stored as `topic`.** The four kinds above are exhaustive;
  if the model returns something else, the daemon files it under `topic`.
- **Merge is per-kind.** You fold variants of one kind into a canonical value of
  the same kind; it doesn't cross kinds.
- Deleting a recording deletes its entities with it; entities travel with library
  exports (see [Exporting & Backup](exporting_and_backup.md)).

## See also

Entities are one of Phoneme's per-recording AI enrichments. The others follow the
same opt-in, hand-edit-survives pattern:

- [Tasks from Voice](tasks_and_reminders.md) — action items pulled into a
  checkable list.
- [Topic Timelines (Auto-Chapters)](topic_timelines.md) — a navigable,
  time-coded chapter list per recording.
