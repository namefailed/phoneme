-- Per-recording machine transcript segments with audio-relative timing.
--
-- Captured from the transcription provider (whisper verbose_json segments,
-- Deepgram word groups, AssemblyAI utterances) on every transcribe /
-- retranscribe, replacing any previous rows for the recording. These are
-- machine truth like `original_transcript` — user edits to the live
-- transcript never touch them. They exist to power timeline views: per-track
-- timing, transcript↔waveform seek, and the chronological meeting merge.
--
-- `speaker` is the label text exactly as it appears in the transcript's
-- `[Speaker …]` marker ("1", "0", "A" — providers differ), NULL for
-- undiarized segments. Numeric labels join against `speaker_names`.
CREATE TABLE IF NOT EXISTS transcript_segments (
    recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    idx          INTEGER NOT NULL,
    start_ms     INTEGER NOT NULL,
    end_ms       INTEGER NOT NULL,
    text         TEXT    NOT NULL,
    speaker      TEXT,
    PRIMARY KEY (recording_id, idx)
);
