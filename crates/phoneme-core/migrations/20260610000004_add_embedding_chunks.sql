-- Per-chunk semantic embeddings.
--
-- The original `embeddings` table stored ONE mean-pooled vector per recording,
-- which (a) drops everything past the model's 256-token window and (b) smears
-- many distinct ideas into a single averaged vector, badly hurting paraphrase
-- recall on longer notes. This table stores MANY vectors per recording — one
-- per sentence-aware chunk — so a spoken idea is matched on its own tight vector
-- (max-sim across a recording's chunks) instead of being diluted.
--
-- The legacy `embeddings` table is left in place so older rows remain searchable
-- until the daemon's re-embed pass backfills chunks for them; the search path
-- prefers chunk embeddings and falls back to the whole-recording vector.

CREATE TABLE embedding_chunks (
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    -- 0-based position of this chunk within the recording's transcript, so
    -- re-embedding can replace a recording's chunks deterministically.
    chunk_index  INTEGER NOT NULL,
    vector       BLOB NOT NULL,
    PRIMARY KEY (recording_id, chunk_index)
);

-- Search loads every chunk and groups by recording; an index on recording_id
-- keeps the delete-before-reinsert (re-embed) path and any per-recording lookup
-- cheap.
CREATE INDEX idx_embedding_chunks_recording ON embedding_chunks(recording_id);
