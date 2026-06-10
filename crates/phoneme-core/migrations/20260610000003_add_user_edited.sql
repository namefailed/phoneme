-- Track whether a recording's transcript was hand-edited by the user, as a
-- dedicated flag (surfaced as an "Edited" column, like `diarized`). Previously a
-- user edit overwrote the `model` column with the literal "user-edit", which
-- destroyed the record of which transcription model actually ran. Now `model`
-- always reflects the transcription model and this flag records the edit.
ALTER TABLE recordings ADD COLUMN user_edited INTEGER NOT NULL DEFAULT 0;
