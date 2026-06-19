-- Speaker voiceprints for cross-recording named-speaker recognition (#9).
--
-- speaker_voiceprints: the raw per-recording, per-speaker centroid embedding
-- captured at local-diarization time (L2-normalized; one row per labelled
-- speaker in a recording, keyed by the same integer speaker_label the
-- speaker_names table and the transcript's [Speaker N] markers use).
-- `named_voice_id` links a capture to a library entry once the user names that
-- speaker; NULL = captured but not yet enrolled.
--
-- named_voiceprints: the cross-recording library — one entry per named voice,
-- with a cached mean centroid (the L2-normalized mean of its linked captures)
-- and a sample count, so recognition can match a new capture without re-reading
-- every sample. The cached centroid is recomputed from the linked captures on
-- every enroll / merge / forget.
CREATE TABLE IF NOT EXISTS named_voiceprints (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    centroid   TEXT NOT NULL,            -- JSON array of f32 (cached mean, L2-normalized)
    samples    INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS speaker_voiceprints (
    recording_id   TEXT NOT NULL,
    speaker_label  INTEGER NOT NULL,
    centroid       TEXT NOT NULL,         -- JSON array of f32 (L2-normalized)
    named_voice_id TEXT,                  -- → named_voiceprints.id; NULL = unenrolled
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (recording_id, speaker_label)
);

CREATE INDEX IF NOT EXISTS idx_speaker_voiceprints_named
    ON speaker_voiceprints(named_voice_id);
