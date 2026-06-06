-- TODO: This migration can be safely removed/squashed in a version or two (like v1.9 or v2.0)
-- since the userbase is very small at the moment.
-- Rename session_id to meeting_id
ALTER TABLE recordings RENAME COLUMN session_id TO meeting_id;

-- Rename session_name to meeting_name
ALTER TABLE recordings RENAME COLUMN session_name TO meeting_name;

-- Drop the old index and recreate it with the new column name
DROP INDEX IF EXISTS idx_recordings_session_id;
CREATE INDEX idx_recordings_meeting_id ON recordings(meeting_id);
