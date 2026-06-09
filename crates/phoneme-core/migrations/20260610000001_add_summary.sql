-- Auto AI summary: per-recording LLM summary, generated on demand or as the
-- final pipeline step. Nullable; absent until generated.
ALTER TABLE recordings ADD COLUMN summary TEXT;
ALTER TABLE recordings ADD COLUMN summary_model TEXT;
