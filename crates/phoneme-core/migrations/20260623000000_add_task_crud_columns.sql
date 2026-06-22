-- Task CRUD foundation (Phase 1 of the tasks/entities overhaul) — two columns so
-- tasks become first-class, user-owned objects rather than a read-only LLM dump:
--
--   • source — 'llm' (pulled by the extraction step) or 'manual' (added by the
--     user). Re-extraction replaces ONLY the 'llm' rows, so a hand-added task is
--     never wiped by a re-run. Every existing row was LLM-extracted, hence the
--     'llm' default.
--   • sort_order — the user's chosen order within a recording (drag-to-reorder).
--     Defaults to 0; new rows are appended past the current max.
ALTER TABLE tasks ADD COLUMN source TEXT NOT NULL DEFAULT 'llm';
ALTER TABLE tasks ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;

-- Ordered per-recording reads: done first, then the user's order, then id.
CREATE INDEX IF NOT EXISTS idx_tasks_sort ON tasks(recording_id, sort_order);
