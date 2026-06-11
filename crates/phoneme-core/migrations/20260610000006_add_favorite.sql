-- Per-recording "favorite"/star flag. Lets users star recordings from the list
-- and filter to a Favorites view in the Library sidebar. Cosmetic organisation
-- only — never affects transcription or the pipeline. Defaults to 0 (not starred).
ALTER TABLE recordings ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;
