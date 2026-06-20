-- Tombstones for dismissed named-speaker suggestions (#9).
--
-- Recognition runs on-demand: when a recording is opened, each diarized speaker
-- that has no name yet is matched against the named-voice library and offered as
-- a suggestion. Once the user dismisses a suggestion for a given (recording,
-- speaker), this records that so it isn't offered again — even as the library
-- grows and the match would otherwise re-appear.
CREATE TABLE IF NOT EXISTS dismissed_speaker_suggestions (
    recording_id  TEXT NOT NULL,
    speaker_label INTEGER NOT NULL,
    PRIMARY KEY (recording_id, speaker_label)
);
