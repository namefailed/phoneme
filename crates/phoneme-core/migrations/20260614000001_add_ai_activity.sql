-- Persistent AI-activity log: one row per completed streaming LLM session
-- (cleanup, summary, and their re-runs — everything that flows through the
-- daemon's `run_llm_stage`). Until now the 🧠 "AI Activity" popout was purely
-- in-memory and reset every time the app reopened ("since the app opened");
-- persisting each session here means the log survives restarts and can be shown
-- per recording (or globally) when the popout opens.
--
-- `recording_id` is NOT a foreign key on purpose: the activity is a historical
-- audit trail, so deleting a recording should NOT silently erase the record that
-- the AI ran on it. The daemon prunes the table to a bounded recent window
-- instead, so it can't grow without limit.

CREATE TABLE ai_activity (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_id TEXT NOT NULL,
    -- The PipelineStage wire value (snake_case, e.g. `cleaning_up`,
    -- `summarizing`) so the frontend renders it with the same `stageLabel()` it
    -- uses for the live events.
    stage        TEXT NOT NULL,
    prompt       TEXT NOT NULL,
    response     TEXT NOT NULL,
    -- RFC3339 UTC timestamp of when the session finished.
    created_at   TEXT NOT NULL
);

-- The popout opens to either a global recent list or one recording's history;
-- both read newest-first, so index the columns each path orders/filters on.
CREATE INDEX idx_ai_activity_created ON ai_activity(created_at);
CREATE INDEX idx_ai_activity_recording ON ai_activity(recording_id);
