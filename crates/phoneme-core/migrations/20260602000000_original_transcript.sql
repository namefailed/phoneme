-- Preserve the original (machine) transcript so a user's manual edits can be
-- reverted. Set by machine transcription; left untouched by user edits.
ALTER TABLE recordings ADD COLUMN original_transcript TEXT;
