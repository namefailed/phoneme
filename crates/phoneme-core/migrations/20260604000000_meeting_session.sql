-- Meeting Mode (v1.6): dual-track capture.
--
-- A meeting records the microphone and the system audio (WASAPI loopback) as
-- two separate recordings. Both rows share a freshly-minted `session_id` so the
-- UI can group them; `track` distinguishes the two ("mic" vs "system").
--
-- Both columns are nullable: every existing recording, and every normal
-- single-track recording going forward, leaves them NULL. Only the two rows
-- produced by a meeting carry values.
ALTER TABLE recordings ADD COLUMN session_id TEXT;
ALTER TABLE recordings ADD COLUMN track TEXT;

CREATE INDEX idx_recordings_session_id ON recordings(session_id);
