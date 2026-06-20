-- Add ON DELETE CASCADE to the voiceprint tables (audit H1). They were created
-- without the foreign key every other recording-scoped table has, so deleting a
-- recording leaked its speaker_voiceprints + dismissed_speaker_suggestions rows
-- forever — and an orphaned, still-enrolled voiceprint kept inflating its named
-- voice's cached centroid. SQLite can't add a foreign key in place, so rebuild the
-- tables; the INSERT filters out any rows already orphaned by deletes before this
-- fix. (The named-voice centroids stay as-is here; `Catalog::delete` now recomputes
-- the affected named voices going forward, and they self-heal on the next enroll.)
CREATE TABLE speaker_voiceprints_new (
    recording_id   TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    speaker_label  INTEGER NOT NULL,
    centroid       TEXT NOT NULL,
    named_voice_id TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (recording_id, speaker_label)
);
INSERT INTO speaker_voiceprints_new (recording_id, speaker_label, centroid, named_voice_id, created_at)
    SELECT recording_id, speaker_label, centroid, named_voice_id, created_at
    FROM speaker_voiceprints
    WHERE recording_id IN (SELECT id FROM recordings);
DROP TABLE speaker_voiceprints;
ALTER TABLE speaker_voiceprints_new RENAME TO speaker_voiceprints;
CREATE INDEX IF NOT EXISTS idx_speaker_voiceprints_named
    ON speaker_voiceprints(named_voice_id);

CREATE TABLE dismissed_speaker_suggestions_new (
    recording_id  TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    speaker_label INTEGER NOT NULL,
    PRIMARY KEY (recording_id, speaker_label)
);
INSERT INTO dismissed_speaker_suggestions_new (recording_id, speaker_label)
    SELECT recording_id, speaker_label FROM dismissed_speaker_suggestions
    WHERE recording_id IN (SELECT id FROM recordings);
DROP TABLE dismissed_speaker_suggestions;
ALTER TABLE dismissed_speaker_suggestions_new RENAME TO dismissed_speaker_suggestions;
