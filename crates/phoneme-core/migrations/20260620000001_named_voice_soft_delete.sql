-- Reversible "forget" for named voices (roadmap V5). Forgetting used to DELETE the
-- library row outright, which is unrecoverable. Add a soft-delete tombstone so a
-- forget can be undone: `forget_named_voice` now stamps `deleted_at` (and stashes
-- the captures it un-linked in the undo log below), and `undo_forget` clears the
-- stamp and re-links those captures. Every read path filters `deleted_at IS NULL`,
-- so a tombstoned voice is invisible to listing and recognition exactly as a hard
-- delete was. Existing rows get NULL = "live", so nothing already in the library
-- changes.
ALTER TABLE named_voiceprints ADD COLUMN deleted_at TEXT;

-- Undo log for a soft-deleted (forgotten) voice: which captures were linked to it
-- at forget time, so undo can re-point exactly those rows back. Keyed per
-- (named_voice_id, recording_id, speaker_label); cleared on undo or on a fresh
-- forget of the same voice. Cascades with its recording so a deleted recording
-- can't leave a dangling undo entry.
CREATE TABLE IF NOT EXISTS forgotten_voiceprint_links (
    named_voice_id TEXT NOT NULL,
    recording_id   TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    speaker_label  INTEGER NOT NULL,
    PRIMARY KEY (named_voice_id, recording_id, speaker_label)
);
CREATE INDEX IF NOT EXISTS idx_forgotten_links_voice
    ON forgotten_voiceprint_links(named_voice_id);
