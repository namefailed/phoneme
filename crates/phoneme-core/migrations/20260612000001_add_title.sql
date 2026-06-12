-- Auto-generated recording titles. `title` is the display title shown in the
-- list and detail header; NULL until generated (the UI falls back to the
-- timestamp). `title_is_auto` tracks ownership: 1 = generated (heuristic/LLM —
-- the pipeline may refresh it on retranscribe), 0 = set by the user (auto
-- writes never overwrite it).
ALTER TABLE recordings ADD COLUMN title TEXT;
ALTER TABLE recordings ADD COLUMN title_is_auto INTEGER NOT NULL DEFAULT 1;
