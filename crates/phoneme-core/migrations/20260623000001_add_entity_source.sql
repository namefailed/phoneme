-- Entity CRUD foundation (Phase 1 of the tasks/entities overhaul). A `source`
-- column distinguishes LLM-extracted entities from user-curated ones so
-- re-extraction replaces ONLY the 'llm' rows — a hand-added, edited, or merged
-- entity is never wiped by a re-run. Every existing row was LLM-extracted, hence
-- the 'llm' default. (Entities are keyed by (recording_id, kind, value); there is
-- no separate sort_order — they group by kind.)
ALTER TABLE entities ADD COLUMN source TEXT NOT NULL DEFAULT 'llm';
