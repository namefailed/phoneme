-- Per-recording models for the remaining pipeline steps, so the detail-pane
-- provenance line can name every step's model (transcription/cleanup/summary
-- were already recorded). All nullable; absent until the relevant step records
-- one. `title_model` and `tag_model` hold the LLM that auto-generated the title
-- / ran the auto-tagger (NULL for a heuristic title or a recording that wasn't
-- auto-tagged). `diarization_model` names a cloud diarizer's model; the local
-- speakrs diarizer has no model name, so it stays NULL (the UI shows "diarized").
ALTER TABLE recordings ADD COLUMN title_model TEXT;
ALTER TABLE recordings ADD COLUMN tag_model TEXT;
ALTER TABLE recordings ADD COLUMN diarization_model TEXT;
