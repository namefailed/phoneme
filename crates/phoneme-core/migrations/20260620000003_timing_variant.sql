-- Cleaned-timing tables for the Timeline/Synced views (compounding,
-- TL-CONSISTENCY). transcript_segments/transcript_words hold the RAW ASR-aligned
-- timeline. After cleanup rewrites the transcript the panel showed the cleaned
-- text but the timeline still showed raw — they diverged. These parallel *_clean
-- tables hold a timeline re-aligned to the post-cleanup text; the view toggles
-- between "raw" (machine truth) and "cleaned".
--
-- Separate tables (not a `variant` column) deliberately: the raw tables' PRIMARY
-- KEY (recording_id, idx) and the U1 speaker-correction queries that update them
-- by (recording_id, idx) stay exactly as they are, and there is NO table rebuild
-- on existing data. Empty until the post-cleanup re-flow writes them; a recording
-- with no cleanup simply has no cleaned rows (a normal state).
CREATE TABLE IF NOT EXISTS transcript_segments_clean (
    recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    idx          INTEGER NOT NULL,
    start_ms     INTEGER NOT NULL,
    end_ms       INTEGER NOT NULL,
    text         TEXT    NOT NULL,
    speaker      TEXT,
    PRIMARY KEY (recording_id, idx)
);

CREATE TABLE IF NOT EXISTS transcript_words_clean (
    recording_id  TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    idx           INTEGER NOT NULL,
    start_ms      INTEGER NOT NULL,
    end_ms        INTEGER NOT NULL,
    text          TEXT    NOT NULL,
    speaker       TEXT,
    confidence    REAL,
    leading_space INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (recording_id, idx)
);
