-- Per-recording "pinned" flag. Lets users pin a recording so it sorts to the
-- top of the library, independent of favorites, and filter to a Pinned view in
-- the Library sidebar. Cosmetic organisation only — never affects transcription
-- or the pipeline. Defaults to 0 (not pinned).
ALTER TABLE recordings ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
