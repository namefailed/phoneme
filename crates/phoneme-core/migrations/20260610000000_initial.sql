-- Phoneme catalog schema - complete initial schema
-- This replaces all incremental migrations for a clean start

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
    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
    original_transcript TEXT,
    notes             TEXT,
    meeting_id        TEXT,
    meeting_name      TEXT,
    track             TEXT,
    in_place          BOOLEAN NOT NULL DEFAULT 0,
    cleanup_model     TEXT,
    diarized          INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_recordings_started_at ON recordings(started_at DESC);
CREATE INDEX idx_recordings_status     ON recordings(status);
CREATE INDEX idx_recordings_meeting_id ON recordings(meeting_id);

-- FTS5 mirror for transcript search
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

-- Tagging support
CREATE TABLE tags (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    name      TEXT NOT NULL UNIQUE,
    color     TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE recording_tags (
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    tag_id       INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (recording_id, tag_id)
);

CREATE INDEX idx_recording_tags_tag_id ON recording_tags(tag_id);

-- Semantic search embeddings
CREATE TABLE embeddings (
    id TEXT PRIMARY KEY,
    vector BLOB NOT NULL,
    FOREIGN KEY(id) REFERENCES recordings(id) ON DELETE CASCADE
);
