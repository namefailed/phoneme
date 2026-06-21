-- Entity extraction: structured, typed entities (person/org/topic/term) pulled
-- from a transcript by an LLM enrichment step, richer than the flat auto-tags.
-- A child table keyed per recording (one row per distinct entity), mirroring the
-- recording_tags shape, plus the per-step model column on `recordings` that the
-- summary/tag steps already have.
--
-- The FK on `recording_id` has ON DELETE CASCADE (the convention the latest FK
-- migration, 20260620000004_forgotten_links_fk.sql, established) so deleting a
-- recording takes its entities with it. Foreign keys are enforced at runtime
-- (the pool sets `foreign_keys=ON`), so the cascade actually fires.
CREATE TABLE entities (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    value        TEXT NOT NULL,
    UNIQUE(recording_id, kind, value)
);

-- Per-recording lookup (the detail view + the row-mapper N+1 query) and the
-- value index that backs the browse-by-value / group-by-kind queries.
CREATE INDEX IF NOT EXISTS idx_entities_recording ON entities(recording_id);
CREATE INDEX IF NOT EXISTS idx_entities_value ON entities(value);

-- The LLM model that produced a recording's entities, recorded for the detail
-- provenance line (mirrors `summary_model` / `tag_model`). NULL for older rows
-- or recordings that were never extracted.
ALTER TABLE recordings ADD COLUMN entities_model TEXT;
