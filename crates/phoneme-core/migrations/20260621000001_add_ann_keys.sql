-- ANN (approximate nearest-neighbour) key map for the optional vector index.
--
-- The optional usearch ANN index (cargo feature `ann-usearch`, config flag
-- `semantic_search.ann.enabled`, both default OFF) addresses one vector by a
-- stable `u64` key. This table is that key ↔ `(recording_id, chunk_index)` map:
-- a usearch search returns keys, which are resolved back to the recording's
-- chunk vector through these rows.
--
-- The table is purely additive — nothing reads it unless the ANN feature is
-- compiled in and the flag is on, and the sidecar index itself is a disposable
-- derived cache (the f32 BLOBs in `embedding_chunks` remain the only source of
-- truth, so a key map drifting from them just triggers a rebuild from SQLite).
--
-- `ON DELETE CASCADE` mirrors the existing embedding FK discipline: deleting a
-- recording drops its key rows automatically, and the ANN `remove(key)` is
-- driven off the same rows so the in-memory graph and this table stay coherent.

CREATE TABLE ann_keys (
    -- The u64 usearch key. AUTOINCREMENT keeps a deleted key from being reused
    -- by a later insert, so a stale sidecar that still references it can be
    -- detected (count drift) and rebuilt rather than silently mismatched.
    key          INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    -- 0-based position of the chunk within the recording, matching
    -- `embedding_chunks.chunk_index`, so a key resolves to exactly one vector.
    chunk_index  INTEGER NOT NULL,
    UNIQUE(recording_id, chunk_index)
);

-- The re-embed and delete paths look up a recording's keys to remove them from
-- the index before re-adding; an index on recording_id keeps that cheap.
CREATE INDEX idx_ann_keys_recording ON ann_keys(recording_id);
