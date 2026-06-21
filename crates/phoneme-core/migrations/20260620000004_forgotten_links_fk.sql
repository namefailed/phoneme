-- Data-integrity (F5): give `forgotten_voiceprint_links.named_voice_id` a real
-- foreign key to `named_voiceprints(id)` with ON DELETE CASCADE. The table was
-- created (20260620000001) with an FK on `recording_id` but only a bare column
-- for `named_voice_id`, so a HARD delete of a named voice (the merge path) could
-- orphan its undo-log rows. `merge_named_voices` already deletes them in code,
-- but the FK makes it structural — any future delete path can't leave a dangling
-- link. Foreign keys are enforced at runtime (the pool sets `foreign_keys=ON`),
-- so this cascade actually fires.
--
-- SQLite can't ADD a constraint in place, so rebuild the table (it's a small
-- ancillary undo log, and nothing references it, so the drop+rename is safe).
-- The INSERT filters any pre-existing orphan rows so it can't fail FK validation
-- mid-migration.
CREATE TABLE forgotten_voiceprint_links_new (
    named_voice_id TEXT NOT NULL REFERENCES named_voiceprints(id) ON DELETE CASCADE,
    recording_id   TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    speaker_label  INTEGER NOT NULL,
    PRIMARY KEY (named_voice_id, recording_id, speaker_label)
);

INSERT INTO forgotten_voiceprint_links_new (named_voice_id, recording_id, speaker_label)
SELECT named_voice_id, recording_id, speaker_label
FROM forgotten_voiceprint_links
WHERE named_voice_id IN (SELECT id FROM named_voiceprints)
  AND recording_id IN (SELECT id FROM recordings);

DROP TABLE forgotten_voiceprint_links;
ALTER TABLE forgotten_voiceprint_links_new RENAME TO forgotten_voiceprint_links;

CREATE INDEX IF NOT EXISTS idx_forgotten_links_voice
    ON forgotten_voiceprint_links(named_voice_id);
