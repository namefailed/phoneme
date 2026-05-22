-- v1.1: tagging support

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
