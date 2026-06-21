-- Topic timelines / auto-chapters: time-ranged chapter rows over one recording's
-- transcript, derived by an LLM enrichment step from the recording's segment
-- timing. A chapter is a `(start_ms, end_ms, title, summary)` span; the LLM is
-- anchored to the recording's real segment start times, snapped server-side, so
-- the boundaries always land on the audio (see `parse_chapters`). A child table
-- keyed per recording (one row per chapter, ordered by `idx`), mirroring the
-- `transcript_segments` shape but with a `title` instead of a `speaker`, plus the
-- per-step model column on `recordings` the summary/entities steps already have.
--
-- The FK on `recording_id` has ON DELETE CASCADE (the convention the entities +
-- segments tables established), so deleting a recording takes its chapters with
-- it. Foreign keys are enforced at runtime (the pool sets `foreign_keys=ON`), so
-- the cascade actually fires. Chapters are replaced wholesale on each run, so
-- re-running overwrites cleanly with no partial-merge logic.
CREATE TABLE chapters (
    recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    idx          INTEGER NOT NULL,
    start_ms     INTEGER NOT NULL,
    end_ms       INTEGER NOT NULL,
    title        TEXT    NOT NULL,
    summary      TEXT,
    PRIMARY KEY (recording_id, idx)
);

-- Per-recording lookup (the detail view fetches a recording's chapters in `idx`
-- order, like `segments_for`).
CREATE INDEX IF NOT EXISTS idx_chapters_recording ON chapters(recording_id);

-- The LLM model that produced a recording's chapters, recorded for the detail
-- provenance line (mirrors `summary_model` / `entities_model`). NULL for older
-- rows or recordings that were never chaptered.
ALTER TABLE recordings ADD COLUMN chapters_model TEXT;
