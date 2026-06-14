-- Per-word "starts a new written word" flag (whisper's leading-space marker).
-- The Synced (per-word) view rebuilds text from these rows; without the flag it
-- space-joins every token and shows "I don 't know" / "over ste pped". Existing
-- rows default to 1 (a normal space-separated word) — correct for clean-word
-- providers and harmless for old whisper rows until they are re-transcribed.
ALTER TABLE transcript_words ADD COLUMN leading_space INTEGER NOT NULL DEFAULT 1;
