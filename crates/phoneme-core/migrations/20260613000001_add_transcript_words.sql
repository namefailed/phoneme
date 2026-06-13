-- Per-recording machine transcript words: the finest timing layer.
--
-- Captured from the transcription provider (whisper `verbose_json` words,
-- Deepgram words, AssemblyAI words) on every transcribe / retranscribe,
-- replacing any previous rows for the recording. Like `transcript_segments`
-- these are machine truth — user edits to the live transcript never touch
-- them. They exist to power the finer word-level timeline features:
-- transcript↔waveform word seek and confidence highlighting.
--
-- `confidence` is the provider's 0..1 per-word score, NULL when the provider
-- gives none (whisper-family endpoints emit only segment-level logprobs).
-- NULL is the graceful-degradation switch — a NULL word renders no confidence
-- styling, while the timing still drives seek. NULL and 0.0 are distinct:
-- provider-absent must be NULL, never a misleading "lowest confidence".
--
-- `speaker` mirrors the segment label exactly as it appears in the
-- transcript's `[Speaker …]` marker, NULL for undiarized words.
CREATE TABLE IF NOT EXISTS transcript_words (
    recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    idx          INTEGER NOT NULL,
    start_ms     INTEGER NOT NULL,
    end_ms       INTEGER NOT NULL,
    text         TEXT    NOT NULL,
    speaker      TEXT,
    confidence   REAL,
    PRIMARY KEY (recording_id, idx)
);
CREATE INDEX IF NOT EXISTS idx_words_recording_time
    ON transcript_words(recording_id, start_ms);
