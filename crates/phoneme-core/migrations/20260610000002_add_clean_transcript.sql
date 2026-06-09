-- "View unedited transcript": the transcript exactly as the pipeline produced
-- it (machine transcription + any LLM cleanup) BEFORE the user made hand edits.
-- Distinct from `original_transcript` (raw machine output, pre-cleanup) and from
-- `transcript` (the current, possibly user-edited text). Nullable; absent for
-- recordings transcribed before this column existed.
ALTER TABLE recordings ADD COLUMN clean_transcript TEXT;
