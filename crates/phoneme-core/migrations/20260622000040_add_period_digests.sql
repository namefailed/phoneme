-- Period digest: one LLM-generated rollup across EVERY recording in a date
-- window (what was discussed, decisions reached, open/action items), distinct
-- from the per-recording `summary` and the whole-meeting digest (which is keyed
-- by `meeting_id`). A period digest spans many independent recordings selected
-- by a `since..until` range, so it has no parent row to hang off — like
-- `meeting_digests`, it lives in its own keyed table.
--
-- The key is derived from the canonical (daemon-normalized) `since|until`
-- RFC3339 bounds, so re-running the same window upserts in place rather than
-- accumulating near-duplicate rows. `label` is the human period name shown in
-- the UI ("2026-06-21", "week of 2026-06-15"); two different ranges can share a
-- label, so the table is keyed on the range, never the label. `source_count`
-- records how many recordings were rolled up (for the "N recordings" line).
-- Nullable `digest_model` mirrors `meeting_digests.digest_model`.
CREATE TABLE IF NOT EXISTS period_digests (
    key          TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    since        TEXT NOT NULL,
    until        TEXT NOT NULL,
    digest       TEXT NOT NULL,
    digest_model TEXT,
    source_count INTEGER NOT NULL DEFAULT 0,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
