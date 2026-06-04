-- Free-form, per-recording notes. Stored separately from the transcript and
-- never overwritten by re-transcription or AI post-processing. Set only by
-- explicit user edits via `update_notes`.
ALTER TABLE recordings ADD COLUMN notes TEXT;
