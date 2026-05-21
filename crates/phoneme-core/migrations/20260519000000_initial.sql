-- Phoneme catalog schema v1

CREATE TABLE recordings (
    id                TEXT PRIMARY KEY,
    started_at        TEXT NOT NULL,
    duration_ms       INTEGER NOT NULL,
    audio_path        TEXT NOT NULL,
    transcript        TEXT,
    model             TEXT,
    status            TEXT NOT NULL,
    error_kind        TEXT,
    error_message     TEXT,
    hook_command      TEXT,
    hook_exit_code    INTEGER,
    hook_duration_ms  INTEGER,
    transcribed_at    TEXT,
    hook_ran_at       TEXT,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_recordings_started_at ON recordings(started_at DESC);
CREATE INDEX idx_recordings_status     ON recordings(status);

-- FTS5 mirror for transcript search.
CREATE VIRTUAL TABLE recordings_fts USING fts5(
    id UNINDEXED,
    transcript,
    content='recordings',
    content_rowid='rowid'
);

CREATE TRIGGER recordings_ai AFTER INSERT ON recordings BEGIN
    INSERT INTO recordings_fts(rowid, id, transcript)
        VALUES (new.rowid, new.id, new.transcript);
END;

CREATE TRIGGER recordings_au AFTER UPDATE ON recordings BEGIN
    INSERT INTO recordings_fts(recordings_fts, rowid, id, transcript)
        VALUES('delete', old.rowid, old.id, old.transcript);
    INSERT INTO recordings_fts(rowid, id, transcript)
        VALUES (new.rowid, new.id, new.transcript);
END;

CREATE TRIGGER recordings_ad AFTER DELETE ON recordings BEGIN
    INSERT INTO recordings_fts(recordings_fts, rowid, id, transcript)
        VALUES('delete', old.rowid, old.id, old.transcript);
END;
