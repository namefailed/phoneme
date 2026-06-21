-- A short, bounded ring buffer of recent in-place dictations — the text that was
-- typed/pasted at the cursor — so a user can re-insert or re-copy a previous one.
-- Text only: no audio path and no FK to recordings, so ephemeral dictations
-- (save_to_library = false, which leave no recording row) are covered too, and a
-- deleted recording never orphans or cascades a history row. The table is pruned
-- to the newest N rows on every insert (see DICTATION_HISTORY_KEEP), like
-- ai_activity, so it can't grow without bound. Opt-in: nothing is written unless
-- [in_place].keep_history is on.
CREATE TABLE IF NOT EXISTS dictation_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    text        TEXT NOT NULL,
    char_count  INTEGER NOT NULL,
    app         TEXT,            -- focused app exe stem at type time, when known (nullable)
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_dictation_history_created ON dictation_history(id DESC);
