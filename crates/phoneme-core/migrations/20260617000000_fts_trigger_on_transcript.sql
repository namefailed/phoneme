-- Scope the FTS update trigger to the only column the index sources.
--
-- The original `recordings_au` trigger fired AFTER UPDATE ON recordings — i.e.
-- on ANY column change. A recording's row is updated many times during its
-- lifetime for non-transcript reasons (status transitions, title, summary,
-- favorite, notes, hook results, tag suggestions, …), and each one paid a full
-- delete + reinsert into the FTS5 index even though the indexed `transcript`
-- never changed. That is pure write amplification on the search index.
--
-- recordings_fts indexes only `transcript` (id is UNINDEXED), so `OF transcript`
-- is the complete column list — no indexed column stops being maintained. Drop
-- and recreate the trigger scoped to that column. The INSERT/DELETE triggers are
-- unchanged.
DROP TRIGGER IF EXISTS recordings_au;

CREATE TRIGGER recordings_au AFTER UPDATE OF transcript ON recordings BEGIN
    INSERT INTO recordings_fts(recordings_fts, rowid, id, transcript)
        VALUES('delete', old.rowid, old.id, old.transcript);
    INSERT INTO recordings_fts(rowid, id, transcript)
        VALUES (new.rowid, new.id, new.transcript);
END;
