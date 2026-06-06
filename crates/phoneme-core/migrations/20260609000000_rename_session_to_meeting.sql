-- Rename session_id to meeting_id
ALTER TABLE recordings RENAME COLUMN session_id TO meeting_id;

-- Rename session_name to meeting_name
ALTER TABLE recordings RENAME COLUMN session_name TO meeting_name;

-- Drop the old index and recreate it with the new column name
DROP INDEX IF EXISTS idx_recordings_session_id;
CREATE INDEX idx_recordings_meeting_id ON recordings(meeting_id);
