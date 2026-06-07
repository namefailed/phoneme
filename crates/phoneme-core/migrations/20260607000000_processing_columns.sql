-- v1.8.2: surface per-recording processing metadata in the list view.
--   cleanup_model — the LLM model used for post-processing (NULL if none ran)
--   diarized      — whether speaker diarization was applied (0/1)

ALTER TABLE recordings ADD COLUMN cleanup_model TEXT;
ALTER TABLE recordings ADD COLUMN diarized INTEGER NOT NULL DEFAULT 0;
