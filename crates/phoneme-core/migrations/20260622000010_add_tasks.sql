-- Task / reminder extraction: structured action items pulled from a transcript
-- by an LLM enrichment step. A child table keyed per recording (one row per
-- distinct action item), mirroring the `entities` table shape, plus the per-step
-- model column on `recordings` that the summary/tag/entity steps already have.
--
-- Tasks differ from entities in two ways: a mutable `done` flag (the one
-- user-owned field — entities are read-only) and a free-text `due_hint` (the
-- model's deadline phrase verbatim, e.g. "by Friday"; NOT parsed to a date).
--
-- The FK on `recording_id` has ON DELETE CASCADE (the convention the FK
-- migration 20260620000004_forgotten_links_fk.sql established) so deleting a
-- recording takes its tasks with it. Foreign keys are enforced at runtime (the
-- pool sets `foreign_keys=ON`), so the cascade actually fires.
CREATE TABLE tasks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    text         TEXT NOT NULL,
    due_hint     TEXT,                        -- nullable free-text deadline phrase; NOT a timestamp
    done         INTEGER NOT NULL DEFAULT 0,  -- 0/1 boolean; the one mutable, user-owned field
    UNIQUE(recording_id, text)
);

-- Per-recording lookup (the detail view + the row-mapper N+1 query) and the
-- done index that backs the cross-library "open tasks" facet.
CREATE INDEX IF NOT EXISTS idx_tasks_recording ON tasks(recording_id);
CREATE INDEX IF NOT EXISTS idx_tasks_done ON tasks(done);

-- The LLM model that produced a recording's tasks, recorded for the detail
-- provenance line (mirrors `entities_model` / `summary_model`). NULL for older
-- rows or recordings that were never extracted.
ALTER TABLE recordings ADD COLUMN tasks_model TEXT;
