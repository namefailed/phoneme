-- Per-step transcript versions (compounding Playbook steps, roadmap PB-COMPOUND).
-- Each Transform step in a recipe rewrites the running transcript; this records
-- every step's output so the chain toward a "perfect" transcript is inspectable
-- and revertible (the Compare-versions UI reads these). idx 0 = raw ASR; later
-- rows are each Transform's output, the last being the transcript that landed.
-- Replaced wholesale on each (re)transcription, like transcript_segments, and
-- cascades with the recording. Recordings that predate compounding simply have
-- no rows here — callers treat "no versions" as a normal state.
CREATE TABLE IF NOT EXISTS transcript_versions (
    recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    idx          INTEGER NOT NULL,   -- step order; 0 = raw ASR
    step_id      TEXT,               -- recipe step id (e.g. "cleanup"); NULL for raw
    label        TEXT,               -- display label (e.g. "Cleanup (llama3.2)")
    model        TEXT,               -- model that produced it, if any
    text         TEXT    NOT NULL,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (recording_id, idx)
);

CREATE INDEX IF NOT EXISTS idx_transcript_versions_recording
    ON transcript_versions (recording_id);
