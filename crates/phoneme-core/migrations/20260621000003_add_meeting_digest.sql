-- Whole-meeting digest: one LLM-generated synthesis across ALL tracks of a
-- meeting (mic + system together), distinct from the per-recording `summary`.
--
-- Meetings aren't their own table — they're `recordings` rows sharing a
-- `meeting_id` — so the digest can't hang off a single row (either track may be
-- deleted, and there is no canonical "primary" track). It lives in its own
-- keyed-by-meeting table instead: one row per meeting, holding the digest text
-- and the model that produced it. Nullable model mirrors `summary_model`.
CREATE TABLE IF NOT EXISTS meeting_digests (
    meeting_id   TEXT PRIMARY KEY,
    digest       TEXT NOT NULL,
    digest_model TEXT,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
