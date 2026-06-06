-- Semantic search embeddings
CREATE TABLE embeddings (
    id TEXT PRIMARY KEY,
    vector BLOB NOT NULL,
    FOREIGN KEY(id) REFERENCES recordings(id) ON DELETE CASCADE
);
