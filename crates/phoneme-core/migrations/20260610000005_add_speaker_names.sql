-- Per-recording custom names for diarized speaker labels ("Named speakers").
--
-- Diarization embeds stable 1-based `[Speaker N]:` markers into the transcript
-- text (see diarization.rs / the Deepgram + AssemblyAI providers). This table
-- maps a recording's speaker index to a user-chosen display name (e.g. 1 →
-- "Sarah"), so a rename is applied at DISPLAY/EXPORT time and the stored
-- transcript keeps its canonical `[Speaker N]` form — renames stay reversible
-- and never corrupt the original text. This mapping is the source of truth.
--
-- Keyed by (recording_id, speaker_label): one name per speaker index per
-- recording. Clearing a name deletes the row (the label falls back to the
-- default "Speaker N"). Rows are cascade-deleted with their recording.

CREATE TABLE speaker_names (
    recording_id  TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    -- 1-based speaker index matching the `[Speaker N]` marker in the transcript.
    speaker_label INTEGER NOT NULL,
    name          TEXT NOT NULL,
    PRIMARY KEY (recording_id, speaker_label)
);

-- Names are always looked up per recording; the PK already covers that, but an
-- explicit index keeps the per-recording fetch cheap and intent clear.
CREATE INDEX idx_speaker_names_recording ON speaker_names(recording_id);
