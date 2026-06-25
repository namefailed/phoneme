-- External-reference key for idempotent import.
--
-- A client (e.g. the youtube-note sister project) sets `ext_ref` to its own
-- stable id for the source it imported. A later import carrying the same
-- `ext_ref` is a no-op that returns the existing recording, so a client can
-- reconcile against the library (`phoneme list --json` exposes `ext_ref`) and
-- fire-and-forget without its own dedup bookkeeping.
--
-- Additive and nullable: existing rows get NULL and nothing is rewritten. The
-- partial index keeps the dedup lookup fast without indexing the (common) NULLs.
ALTER TABLE recordings ADD COLUMN ext_ref TEXT;
CREATE INDEX IF NOT EXISTS idx_recordings_ext_ref ON recordings(ext_ref) WHERE ext_ref IS NOT NULL;
