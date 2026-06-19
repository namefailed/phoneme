-- Saved searches move from the webview's localStorage into the catalog, so they
-- persist across a reinstall and can ride catalog sync later. A saved search is a
-- user-named snapshot of the full library filter; the filter is stored as the
-- opaque JSON the frontend serializes (the daemon never interprets it — it only
-- stores and returns it). Names are unique case-insensitively, enforced in the
-- upsert (a re-save under a known name updates in place), not by a DB constraint,
-- to match the existing frontend semantics.
CREATE TABLE IF NOT EXISTS saved_searches (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    filter_json TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
