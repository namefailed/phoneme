//! Embedding cache internals plus embedding storage and hybrid search.

use super::*;

impl Catalog {
    /// Drop the in-memory embedding snapshot. Called from every path that mutates
    /// a stored vector so the next search rebuilds from SQLite. A poisoned lock is
    /// recovered (cleared) rather than propagated: the dangerous outcome is
    /// failing to invalidate (stale rankings), so we clear either way.
    ///
    /// The generation counter is bumped while holding the cache write lock, so it
    /// is ordered against a racing `embedding_corpus` store. That store snapshots
    /// the generation before its SQL reads and re-checks it under this same lock
    /// before caching, so an invalidation landing between the snapshot's read and
    /// its store is observed (via the bump) and the store backs off — the writer's
    /// invalidation can't be silently clobbered.
    pub(crate) fn invalidate_embedding_cache(&self) {
        match self.embedding_cache.write() {
            Ok(mut guard) => {
                self.embedding_cache_gen.fetch_add(1, Ordering::Release);
                *guard = None;
            }
            Err(poisoned) => {
                self.embedding_cache_gen.fetch_add(1, Ordering::Release);
                *poisoned.into_inner() = None;
            }
        }
    }

    /// The decoded embedding corpus, loaded once and held until the next write
    /// invalidates it.
    ///
    /// On a hit, returns the cached snapshot by cloning its `Arc` (O(1) — no deep
    /// copy of the vectors). The ranking loops consume the corpus by reference,
    /// so the shared `Arc<EmbeddingCorpus>` serves them all without copying. On a
    /// miss, reads both embedding tables, decodes every blob once, and caches the
    /// result unless it exceeds [`MAX_CACHED_VECTORS`] (in which case the corpus
    /// is returned but not stored, keeping memory bounded) — or unless an
    /// invalidation raced the rebuild, in which case the freshly-read snapshot is
    /// returned but the slot is left cold so the racing writer's view wins (see
    /// the generation guard below).
    pub(crate) async fn embedding_corpus(&self) -> Result<Arc<EmbeddingCorpus>> {
        // Fast path: a warm snapshot. Read lock only; clone the Arc (O(1)).
        if let Ok(guard) = self.embedding_cache.read() {
            if let Some(corpus) = guard.as_ref() {
                return Ok(corpus.clone());
            }
        }

        // Snapshot the generation before the SQL reads. An invalidation that runs
        // while we read and decode below bumps this counter (under the cache write
        // lock), so the store step sees the mismatch and declines to cache — a
        // vector changed mid-rebuild can't be masked by a snapshot taken before
        // that change committed.
        let gen_at_miss = self.embedding_cache_gen.load(Ordering::Acquire);

        // Miss: rebuild from SQLite. Decode happens outside any lock.
        let chunk_rows = sqlx::query(
            "SELECT ec.recording_id AS id, ec.vector AS vector, r.meeting_id AS meeting_id \
             FROM embedding_chunks ec JOIN recordings r ON r.id = ec.recording_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut chunks = Vec::with_capacity(chunk_rows.len());
        for row in chunk_rows {
            chunks.push(Arc::new(row_to_cached_vector(&row)?));
        }

        let legacy_rows = sqlx::query(
            "SELECT e.id AS id, e.vector AS vector, r.meeting_id AS meeting_id \
             FROM embeddings e JOIN recordings r ON r.id = e.id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut legacy = Vec::with_capacity(legacy_rows.len());
        for row in legacy_rows {
            legacy.push(Arc::new(row_to_cached_vector(&row)?));
        }

        let corpus = Arc::new(EmbeddingCorpus { chunks, legacy });
        self.store_corpus_if_current(corpus.clone(), gen_at_miss);
        Ok(corpus)
    }

    /// Cache `corpus` under the write lock, but only when the generation still
    /// matches `gen_at_miss` (the value snapshotted before the rebuild's SQL
    /// reads) and the corpus is under [`MAX_CACHED_VECTORS`].
    ///
    /// This is the store half of the lost-invalidation guard. Holding the write
    /// lock here orders the generation re-read against `invalidate_embedding_cache`
    /// (which bumps the generation under the same lock), so an invalidation that
    /// raced the rebuild shows up as a mismatch and the slot is left cold — the
    /// racing writer's view wins instead of being clobbered by a snapshot taken
    /// before its write committed.
    pub(crate) fn store_corpus_if_current(&self, corpus: Arc<EmbeddingCorpus>, gen_at_miss: u64) {
        // Large libraries stay uncached so memory can't grow without bound.
        if !Self::cap_allows_caching(corpus.chunks.len() + corpus.legacy.len()) {
            return;
        }
        let mut guard = match self.embedding_cache.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if self.embedding_cache_gen.load(Ordering::Acquire) == gen_at_miss {
            *guard = Some(corpus);
        }
    }

    /// Decode just one recording's vectors (its chunks plus its legacy
    /// whole-recording row) from SQLite — the targeted read behind
    /// `patch_recording_in_cache`, so a single embed or delete touches only this
    /// recording's blobs instead of the whole corpus.
    async fn load_recording_vectors(
        &self,
        id: &RecordingId,
    ) -> Result<(Vec<Arc<CachedVector>>, Vec<Arc<CachedVector>>)> {
        let chunk_rows = sqlx::query(
            "SELECT ec.recording_id AS id, ec.vector AS vector, r.meeting_id AS meeting_id \
             FROM embedding_chunks ec JOIN recordings r ON r.id = ec.recording_id \
             WHERE ec.recording_id = ?",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;
        let mut chunks = Vec::with_capacity(chunk_rows.len());
        for row in chunk_rows {
            chunks.push(Arc::new(row_to_cached_vector(&row)?));
        }

        let legacy_rows = sqlx::query(
            "SELECT e.id AS id, e.vector AS vector, r.meeting_id AS meeting_id \
             FROM embeddings e JOIN recordings r ON r.id = e.id WHERE e.id = ?",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;
        let mut legacy = Vec::with_capacity(legacy_rows.len());
        for row in legacy_rows {
            legacy.push(Arc::new(row_to_cached_vector(&row)?));
        }
        Ok((chunks, legacy))
    }

    /// Patch a single recording's vectors into the warm embedding cache instead of
    /// dropping the whole snapshot. Reads only this recording's rows, then
    /// copy-on-writes a fresh corpus that shares every other recording's vectors by
    /// `Arc` pointer. Used after a single embed or delete; bulk ops (clear-all,
    /// retention sweep) still invalidate coarsely.
    ///
    /// A cold cache is left cold (the next query rebuilds from SQLite, now
    /// including this change). A patch that pushes past the cap drops to uncached.
    /// Any read error falls back to a full invalidation, so a stale vector can
    /// never be served. The generation bump makes a full rebuild that snapshotted
    /// an older generation back off, just as a coarse invalidation does.
    ///
    /// Race guard (mirrors the rebuild path): the loaded vectors are a snapshot of
    /// SQLite taken outside the cache lock, so if any other write — another patch
    /// or an invalidation — lands between the load and the store, this corpus can't
    /// be trusted (a concurrent same-id patch could otherwise lose its update). We
    /// snapshot the generation before the load and, under the write lock, drop to a
    /// coarse invalidation instead of writing a possibly-stale copy-on-write when
    /// it moved. The common, uncontended case still does the cheap incremental
    /// patch.
    pub(crate) async fn patch_recording_in_cache(&self, id: &RecordingId) {
        let gen_at_load = self.embedding_cache_gen.load(Ordering::Acquire);
        let (new_chunks, new_legacy) = match self.load_recording_vectors(id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(id = %id.as_str(), error = %e, "embedding cache: targeted reload failed; dropping snapshot");
                self.invalidate_embedding_cache();
                return;
            }
        };
        let id_str = id.as_str();
        let mut guard = match self.embedding_cache.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        // A write raced our load (the generation moved): our loaded vectors may be
        // stale relative to the current cache, so don't copy-on-write from it. Bump
        // and drop to cold; the next query rebuilds from SQLite, which holds the
        // latest committed rows. The bump also makes any in-flight rebuild back off.
        let raced = self.embedding_cache_gen.load(Ordering::Acquire) != gen_at_load;
        // Bump under the lock so a rebuild that snapshotted an older generation
        // (it may have read SQLite before this change committed) declines to cache.
        self.embedding_cache_gen.fetch_add(1, Ordering::Release);
        if raced {
            *guard = None;
            return;
        }
        let Some(corpus) = guard.as_ref() else {
            return; // cold: next query rebuilds from SQLite, including this change
        };
        let mut chunks: Vec<Arc<CachedVector>> = corpus
            .chunks
            .iter()
            .filter(|cv| cv.id != id_str)
            .cloned()
            .collect();
        chunks.extend(new_chunks);
        let mut legacy: Vec<Arc<CachedVector>> = corpus
            .legacy
            .iter()
            .filter(|cv| cv.id != id_str)
            .cloned()
            .collect();
        legacy.extend(new_legacy);
        if Self::cap_allows_caching(chunks.len() + legacy.len()) {
            *guard = Some(Arc::new(EmbeddingCorpus { chunks, legacy }));
        } else {
            *guard = None; // grew past the cap → fall back to uncached
        }
    }

    /// Whether a corpus of `total_vectors` is small enough to cache in memory.
    /// The single source of truth for the [`MAX_CACHED_VECTORS`] bound, so the
    /// loader and the test that proves boundedness agree by construction.
    pub(crate) fn cap_allows_caching(total_vectors: usize) -> bool {
        total_vectors <= MAX_CACHED_VECTORS
    }

    /// Test-only view of the embedding cache: the number of vectors currently
    /// held in the warm snapshot, or `None` when the snapshot is cold (never
    /// loaded, invalidated, or skipped for being over the cap). Lets the cache
    /// tests assert warm/cold state and the bound without exposing internals.
    #[cfg(test)]
    pub(crate) fn cached_vector_count(&self) -> Option<usize> {
        self.embedding_cache
            .read()
            .unwrap()
            .as_ref()
            .map(|c| c.chunks.len() + c.legacy.len())
    }

    /// Upsert the semantic embedding vector for a recording.
    pub async fn upsert_embedding(&self, id: &RecordingId, vector: &[f32]) -> Result<()> {
        // Pack the f32 array into little-endian bytes.
        let mut bytes = Vec::with_capacity(vector.len() * 4);
        for &v in vector {
            bytes.extend_from_slice(&v.to_le_bytes());
        }

        sqlx::query(
            "INSERT INTO embeddings (id, vector) VALUES (?, ?)
             ON CONFLICT(id) DO UPDATE SET vector = excluded.vector",
        )
        .bind(id.as_str())
        .bind(bytes)
        .execute(&self.pool)
        .await?;

        // A vector changed — patch just this recording into the warm cache, with
        // no whole-corpus re-decode. A stale cached vector would rank wrongly.
        self.patch_recording_in_cache(id).await;
        Ok(())
    }

    /// Recordings with a transcript but no legacy whole-recording embedding yet.
    /// Drives the embedding backfill for the older `embeddings` table.
    pub async fn list_recordings_without_embeddings(&self) -> Result<Vec<Recording>> {
        let rows = sqlx::query(
            "SELECT * FROM recordings \
             WHERE id NOT IN (SELECT id FROM embeddings) \
             AND transcript IS NOT NULL AND transcript != ''",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_recording).collect()
    }

    /// Replace all chunk embeddings for a recording in one transaction.
    ///
    /// Per-chunk embeddings (one vector per sentence-aware chunk) are what make
    /// paraphrase recall work on longer notes — see [`crate::chunk`]. Re-embedding
    /// deletes the recording's existing chunks first so a re-transcription or an
    /// edit can't leave stale vectors from the previous text behind. An empty
    /// `vectors` (e.g. a blank transcript) just clears the chunks.
    pub async fn upsert_chunk_embeddings(
        &self,
        id: &RecordingId,
        vectors: &[Vec<f32>],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM embedding_chunks WHERE recording_id = ?")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        for (idx, vector) in vectors.iter().enumerate() {
            let mut bytes = Vec::with_capacity(vector.len() * 4);
            for &v in vector {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            sqlx::query(
                "INSERT INTO embedding_chunks (recording_id, chunk_index, vector) VALUES (?, ?, ?)",
            )
            .bind(id.as_str())
            .bind(idx as i64)
            .bind(bytes)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        // A recording's chunk vectors were replaced — patch just it into the warm
        // cache instead of dropping the whole snapshot.
        self.patch_recording_in_cache(id).await;
        Ok(())
    }

    /// Delete all stored embeddings — per-chunk and legacy whole-recording — so
    /// the whole library can be re-embedded with a newly-configured model. After
    /// this, every recording counts as "without chunk embeddings", so the daemon's
    /// backfill re-embeds them. Vectors from a different model or dimension would
    /// otherwise be silently skipped by the dimension guard, leaving the recording
    /// unsearchable until re-embedded.
    pub async fn clear_all_embeddings(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM embedding_chunks")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM embeddings")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        // Whole library wiped for a re-embed — drop the snapshot.
        self.invalidate_embedding_cache();
        Ok(())
    }

    /// Recordings that have a transcript but no chunk embeddings yet. Drives the
    /// daemon's one-time backfill that migrates the library from the legacy
    /// whole-recording `embeddings` table to per-chunk vectors.
    pub async fn list_recordings_without_chunk_embeddings(&self) -> Result<Vec<Recording>> {
        let rows = sqlx::query(
            "SELECT * FROM recordings \
             WHERE id NOT IN (SELECT DISTINCT recording_id FROM embedding_chunks) \
             AND transcript IS NOT NULL AND transcript != ''",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_recording).collect()
    }

    /// Loads all embeddings into memory for brute-force cosine similarity.
    pub async fn load_all_embeddings(&self) -> Result<Vec<(RecordingId, Vec<f32>)>> {
        let rows = sqlx::query("SELECT id, vector FROM embeddings")
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id")?;
            let bytes: Vec<u8> = row.try_get("vector")?;

            if !bytes.len().is_multiple_of(4) {
                tracing::warn!(
                    "Embedding for {} has invalid byte length: {}",
                    id,
                    bytes.len()
                );
                continue;
            }

            let mut vec = Vec::with_capacity(bytes.len() / 4);
            for chunk in bytes.chunks_exact(4) {
                vec.push(f32::from_le_bytes(
                    chunk
                        .try_into()
                        .expect("chunks_exact(4) yields exactly 4 bytes"),
                ));
            }

            if let Some(rec_id) = RecordingId::parse(id) {
                results.push((rec_id, vec));
            }
        }

        Ok(results)
    }

    /// Semantic search across embedded recordings, returning the top matches as
    /// `(id, cosine_score)` sorted high→low.
    ///
    /// - **Dimension safety:** an embedding whose length doesn't match the query
    ///   vector is skipped — cosine over mismatched dimensions is meaningless and
    ///   would otherwise score on a silently-truncated prefix.
    /// - **Relevance floor:** results scoring below `min_score` are dropped, so a
    ///   vague or garbage query returns few or no results rather than `limit`
    ///   arbitrary ones.
    /// - **Meeting dedupe:** a meeting's two tracks share a `meeting_id` and have
    ///   near-identical transcripts; they collapse to a single best-scoring entry
    ///   so they don't crowd out other recordings. Standalone recordings are keyed
    ///   by their own id.
    pub async fn semantic_search(
        &self,
        query_vec: &[f32],
        limit: usize,
        min_score: f32,
    ) -> Result<Vec<(RecordingId, f32)>> {
        // Read the legacy whole-recording vectors from the shared decoded corpus,
        // so a query doesn't re-read and re-decode the blobs each time.
        let corpus = self.embedding_corpus().await?;

        let dim = query_vec.len();
        let query: Vec<f32> = query_vec.to_vec();

        // The cosine scan over the legacy corpus is CPU-bound — up to
        // MAX_CACHED_VECTORS dot products — so run it on the blocking pool. On a
        // large library or a slow box, doing it inline would starve the async
        // executor (IPC named-pipe reads, audio streaming) between await points.
        let best = tokio::task::spawn_blocking(move || {
            // Best (id, score) per result key — meeting_id when present, else the
            // recording id — so a meeting contributes at most one result.
            let mut best: std::collections::HashMap<String, (RecordingId, f32)> =
                std::collections::HashMap::new();
            for cv in &corpus.legacy {
                let Some(vec) = cv.vector.as_deref() else {
                    continue; // corrupt blob, already warned at load
                };
                if vec.len() != dim {
                    tracing::warn!(id = %cv.id, dim = vec.len(), query_dim = dim, "skipping embedding: dimension mismatch");
                    continue;
                }

                let score = crate::embed::Embedder::cosine_similarity(&query, vec);
                if score < min_score {
                    continue;
                }
                let Some(rec_id) = RecordingId::parse(cv.id.clone()) else {
                    continue;
                };
                let key = cv.meeting_id.clone().unwrap_or_else(|| cv.id.clone());
                best.entry(key)
                    .and_modify(|e| {
                        if score > e.1 {
                            *e = (rec_id.clone(), score);
                        }
                    })
                    .or_insert((rec_id, score));
            }
            best
        })
        .await
        .map_err(|e| crate::error::Error::Internal(format!("semantic search task failed: {e}")))?;

        let mut scores: Vec<(RecordingId, f32)> = best.into_values().collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(limit);
        Ok(scores)
    }

    /// Compute the per-recording best-chunk (max-sim) cosine ranking for a query
    /// vector, meeting-deduped.
    ///
    /// Returns `(dedupe_key, RecordingId, raw_cosine)` sorted high→low. The raw
    /// cosine is of the single best-matching chunk, which is what makes a
    /// paraphrase of one spoken idea rank on that idea instead of on an averaged
    /// whole-note vector. The `dedupe_key` is the recording's `meeting_id` when it
    /// belongs to a meeting, else its own id — exposed so the fusion in
    /// [`Self::hybrid_search`] can collapse a meeting on the same key the lexical
    /// retriever uses, even when the two retrievers each pick a different track of
    /// that meeting as its representative (without it, the meeting would surface
    /// twice). Recordings that only have a legacy whole-recording vector (no chunks
    /// yet, pending backfill) are folded in from the `embeddings` table so nothing
    /// becomes unsearchable during migration. Dimension-mismatched vectors are
    /// skipped (the same guard as [`Self::semantic_search`]).
    pub(crate) async fn vector_ranking(
        &self,
        query_vec: &[f32],
    ) -> Result<Vec<(String, RecordingId, f32)>> {
        let dim = query_vec.len();
        let query: Vec<f32> = query_vec.to_vec();
        // The decoded corpus (cached across queries; rebuilt after any write).
        let corpus = self.embedding_corpus().await?;

        // Best-chunk cosine over the whole corpus is CPU-bound — up to
        // MAX_CACHED_VECTORS dot products — so run it on the blocking pool rather
        // than inline on the async executor, where a large library would stall
        // IPC reads / audio streaming between await points.
        let best = tokio::task::spawn_blocking(move || {
            // best raw cosine per dedupe key (meeting_id or recording id).
            let mut best: std::collections::HashMap<String, (RecordingId, f32)> =
                std::collections::HashMap::new();

            // Score one pre-decoded vector into `best`. A corrupt blob (vector
            // None) or a dimension mismatch is skipped, just as the inline decode
            // did.
            let mut consider = |cv: &CachedVector| {
                let Some(vec) = cv.vector.as_deref() else {
                    return; // corrupt blob, already warned at load
                };
                if vec.len() != dim {
                    tracing::warn!(id = %cv.id, dim = vec.len(), query_dim = dim, "skipping embedding: dimension mismatch");
                    return;
                }
                let score = crate::embed::Embedder::cosine_similarity(&query, vec);
                let Some(rec_id) = RecordingId::parse(cv.id.clone()) else {
                    return;
                };
                let key = cv.meeting_id.clone().unwrap_or_else(|| cv.id.clone());
                best.entry(key)
                    .and_modify(|e| {
                        if score > e.1 {
                            *e = (rec_id.clone(), score);
                        }
                    })
                    .or_insert((rec_id, score));
            };

            // Per-chunk vectors (the primary, high-recall path).
            let mut have_chunks: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            for cv in &corpus.chunks {
                have_chunks.insert(cv.id.as_str());
                consider(cv);
            }

            // Legacy whole-recording vectors, only for recordings not yet chunked,
            // so the library stays searchable while the backfill runs.
            for cv in &corpus.legacy {
                if have_chunks.contains(cv.id.as_str()) {
                    continue; // chunks supersede the legacy whole-recording vector
                }
                consider(cv);
            }
            best
        })
        .await
        .map_err(|e| crate::error::Error::Internal(format!("vector ranking task failed: {e}")))?;

        let mut ranking: Vec<(String, RecordingId, f32)> = best
            .into_iter()
            .map(|(key, (rec_id, score))| (key, rec_id, score))
            .collect();
        ranking.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        Ok(ranking)
    }

    /// The lexical (FTS5) ranking for a query, meeting-deduped, best-first.
    ///
    /// FTS5 `rank` is BM25-like (more negative = more relevant), so ordering by
    /// `rank` ascending gives best-first. We keep the first (best) occurrence per
    /// dedupe key and return `(dedupe_key, RecordingId)` so the fusion in
    /// [`Self::hybrid_search`] collapses a meeting on the same key the vector
    /// retriever uses. This list feeds the RRF fusion as the "exact term"
    /// retriever that complements the paraphrase-oriented vector retriever.
    pub(crate) async fn lexical_ranking(&self, query: &str) -> Result<Vec<(String, RecordingId)>> {
        let sanitized = sanitize_fts5_query(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT r.id AS id, r.meeting_id AS meeting_id \
             FROM recordings_fts f \
             JOIN recordings r ON r.rowid = f.rowid \
             WHERE recordings_fts MATCH ? \
             ORDER BY f.rank",
        )
        .bind(&sanitized)
        .fetch_all(&self.pool)
        .await?;

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let meeting_id: Option<String> = row.try_get("meeting_id")?;
            let key = meeting_id.unwrap_or_else(|| id.clone());
            if !seen.insert(key.clone()) {
                continue; // already have the best-ranked track of this meeting
            }
            if let Some(rec_id) = RecordingId::parse(id) {
                out.push((key, rec_id));
            }
        }
        Ok(out)
    }

    /// Recordings whose tag name matches `query` (case-insensitive substring),
    /// meeting-deduped, in the same `(dedupe_key, RecordingId)` shape as
    /// [`Self::lexical_ranking`]. Feeds the hybrid search's lexical (exact-intent)
    /// side so a tag-name query surfaces its tagged recordings even in semantic
    /// mode, mirroring the tag clause the plain [`Self::list`] already applies.
    async fn tag_ranking(&self, query: &str) -> Result<Vec<(String, RecordingId)>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let like = format!("%{q}%");
        let rows = sqlx::query(
            "SELECT r.id AS id, r.meeting_id AS meeting_id \
             FROM recordings r \
             JOIN recording_tags rt ON rt.recording_id = r.id \
             JOIN tags t ON t.id = rt.tag_id \
             WHERE t.name LIKE ? \
             ORDER BY r.started_at DESC, r.id DESC",
        )
        .bind(&like)
        .fetch_all(&self.pool)
        .await?;

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let meeting_id: Option<String> = row.try_get("meeting_id")?;
            let key = meeting_id.unwrap_or_else(|| id.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(rec_id) = RecordingId::parse(id) {
                out.push((key, rec_id));
            }
        }
        Ok(out)
    }

    /// Hybrid semantic + lexical search with Reciprocal Rank Fusion (RRF).
    ///
    /// This is the search the daemon uses. It merges the ordered listings from two
    /// retrievers:
    /// 1. A vector retriever that ranks by best-matching chunk cosine — cosine
    ///    similarity over ONNX embedding chunks (see [`crate::embed::Embedder`]),
    ///    the paraphrase-recall side.
    /// 2. A lexical retriever: an FTS5 BM25 prefix query over the full-text search
    ///    virtual table, the exact-term-recall side.
    ///
    /// RRF fuses the two without a fragile cross-scale threshold. The result is
    /// `(RecordingId, relevance)`, where `relevance` is the calibrated best-chunk
    /// cosine (0..1) for display — a meaningful percentage rather than a raw
    /// cosine. Lexical-only hits (no vector signal) get a small relevance floor so
    /// they still surface with an honest "weak semantic match" reading.
    ///
    /// ### Meeting collapsing
    /// In Meeting Mode a single meeting has two tracks (microphone and system
    /// loopback). Returning both as separate results would clutter the UI, so
    /// results are grouped by a stable dedupe key (the `meeting_id` for meetings,
    /// the `id` for standalone voice notes). When the two retrievers match
    /// different tracks of the same meeting the results collapse, and a single
    /// representative `RecordingId` comes back, preferring the track with the
    /// strongest semantic match.
    ///
    /// ### Relevance calibration and flooring
    /// `min_relevance` filters out weak semantic hits whose calibrated cosine falls
    /// below the floor. Exact-term matches from the lexical retriever are exempt: a
    /// query for an exact word present in the transcript is returned even when its
    /// semantic similarity is low.
    ///
    /// ### Optional filter (S3)
    /// When `filter` is `Some`, the fused results are restricted to recordings
    /// matching the same constraints as the plain [`Self::list`] — tag, status,
    /// date range, kind, favorite, in-place, tag-presence — so a meaning-search can
    /// be scoped exactly like the Library. The restriction runs after ranking but
    /// before the `limit` truncation, so the top-`limit` in-scope results come back,
    /// not the top-`limit` overall then thinned. A meeting passes when either track
    /// matches (the candidate set is keyed by the same meeting-stable dedupe key).
    /// The filter's query and pagination fields — `search` (the query is the
    /// separate `query`/`query_vec`), `limit`, `offset`, `sort_desc` — are ignored
    /// for the restriction; only its predicate fields scope the candidate set.
    /// `None` leaves the unscoped behavior unchanged.
    pub async fn hybrid_search(
        &self,
        query: &str,
        query_vec: &[f32],
        limit: usize,
        min_relevance: f32,
        filter: Option<&ListFilter>,
    ) -> Result<Vec<(RecordingId, f32)>> {
        // S3: when a filter is given, pre-compute the in-scope dedupe keys so the
        // fused ranking can be restricted to them. Built from the same `list` query
        // the Library uses (predicate fields only — query and pagination dropped),
        // then mapped to dedupe keys so a meeting passes if either of its tracks
        // matches.
        let allowed_keys: Option<std::collections::HashSet<String>> = match filter {
            Some(f) => {
                let scoped = ListFilter {
                    // Drop query, pagination, and sort: this list only derives the
                    // in-scope candidate set, it doesn't order or page it.
                    search: None,
                    limit: None,
                    offset: None,
                    sort_desc: None,
                    ..f.clone()
                };
                let rows = self.list(&scoped).await?;
                Some(
                    rows.into_iter()
                        .map(|r| r.meeting_id.unwrap_or_else(|| r.id.as_str().to_string()))
                        .collect(),
                )
            }
            None => None,
        };

        let vec_rank = self.vector_ranking(query_vec).await?;
        let mut lex_rank = self.lexical_ranking(query).await?;
        // Fold tag-name matches into the lexical (exact-intent) set so searching
        // a tag surfaces its recordings in semantic mode too — the plain `list()`
        // already does this for non-semantic search. Deduped by key and appended
        // after the FTS hits, so true transcript matches keep their stronger rank.
        {
            let mut seen: std::collections::HashSet<String> =
                lex_rank.iter().map(|(k, _)| k.clone()).collect();
            for (key, id) in self.tag_ranking(query).await? {
                if seen.insert(key.clone()) {
                    lex_rank.push((key, id));
                }
            }
        }

        // Everything below is keyed by the meeting-stable dedupe key (meeting_id or
        // recording id), not the raw recording id, so a meeting collapses to a
        // single result even when the vector and lexical retrievers each pick a
        // different track of it as their representative.

        // dedupe_key -> best raw cosine (for calibration into a relevance %).
        let cosine_by_key: std::collections::HashMap<String, f32> = vec_rank
            .iter()
            .map(|(key, _id, c)| (key.clone(), *c))
            .collect();
        // dedupe_key -> a representative RecordingId to return for that key.
        // Prefer the vector retriever's pick (best-chunk track); fall back to the
        // lexical retriever's for lexical-only hits.
        let mut rec_id_by_key: std::collections::HashMap<String, RecordingId> =
            std::collections::HashMap::new();
        for (key, id, _c) in &vec_rank {
            rec_id_by_key
                .entry(key.clone())
                .or_insert_with(|| id.clone());
        }
        for (key, id) in &lex_rank {
            rec_id_by_key
                .entry(key.clone())
                .or_insert_with(|| id.clone());
        }
        let lexical_keys: std::collections::HashSet<String> =
            lex_rank.iter().map(|(key, _id)| key.clone()).collect();

        // Fuse the two orderings on the dedupe key.
        let vec_keys: Vec<String> = vec_rank.iter().map(|(key, _, _)| key.clone()).collect();
        let lex_keys: Vec<String> = lex_rank.iter().map(|(key, _)| key.clone()).collect();
        // Weight the semantic list slightly higher: the whole point is paraphrase
        // recall, and the lexical list is the complementary safety net.
        let fused = crate::fusion::reciprocal_rank_fusion(
            &[&vec_keys[..], &lex_keys[..]],
            Some(&[1.0, 0.85]),
        );

        // Small relevance floor for a lexical-only hit so it surfaces honestly
        // rather than reading "0% relevant" despite being an exact-term match.
        const LEXICAL_ONLY_RELEVANCE: f32 = 0.30;

        let mut results: Vec<(RecordingId, f32)> = Vec::new();
        for (key, _fused_score) in fused {
            // S3: restrict to the in-scope candidate set when a filter was given.
            // Applied here — after ranking, before the `truncate(limit)` below —
            // so the top in-scope results survive rather than the top overall.
            if let Some(allowed) = &allowed_keys {
                if !allowed.contains(&key) {
                    continue;
                }
            }
            let Some(rec_id) = rec_id_by_key.get(&key).cloned() else {
                continue;
            };
            let is_lexical = lexical_keys.contains(&key);
            let relevance = match cosine_by_key.get(&key) {
                Some(c) => crate::fusion::calibrate_cosine(*c),
                None => 0.0,
            };
            // A lexical hit is kept regardless of its (possibly weak) cosine; a
            // semantic-only hit must clear the relevance floor.
            let display = if is_lexical {
                relevance.max(LEXICAL_ONLY_RELEVANCE)
            } else {
                relevance
            };
            if !is_lexical && display < min_relevance {
                continue;
            }
            results.push((rec_id, display));
        }
        results.truncate(limit);
        Ok(results)
    }

    /// "More like this": rank the library by semantic similarity to a stored
    /// recording, reusing its already-stored vectors — no fresh embedding, so
    /// it costs one corpus scan and works even when the embedding model isn't
    /// loaded.
    ///
    /// The query vector is the mean of the source's chunk vectors, L2-renormalized.
    /// The centroid captures what the whole note is about, while candidates are
    /// still scored by their own best-matching chunk via `vector_ranking` — the
    /// same retrieval path a typed semantic query takes — so a long candidate ranks
    /// on its closest idea instead of an averaged blur. A source that only has a
    /// legacy whole-recording vector uses that vector directly, since it already is
    /// that recording's mean.
    ///
    /// The source never appears in the results. Exclusion is by the meeting-stable
    /// dedupe key, so for a meeting track the other track of the same meeting — a
    /// near-identical transcript that would trivially rank #1 — is excluded too.
    /// Scores are calibrated like a normal semantic search
    /// ([`crate::fusion::calibrate_cosine`]); hits under `min_relevance` are
    /// dropped, and at most `limit` results return, best-first.
    ///
    /// Errors: [`crate::error::Error::NotFound`] when `id` doesn't exist, and
    /// a "not indexed yet" [`crate::error::Error::Internal`] when the
    /// recording has no usable stored vectors (not embedded yet, or every
    /// blob was corrupt) — the caller can surface that message as-is.
    pub async fn more_like_this(
        &self,
        id: &RecordingId,
        limit: usize,
        min_relevance: f32,
    ) -> Result<Vec<(RecordingId, f32)>> {
        // Resolve the source row first so a missing id reports NotFound rather
        // than "not indexed", and grab its meeting for the dedupe-key exclusion.
        let row = sqlx::query("SELECT meeting_id FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Err(crate::error::Error::NotFound { id: id.to_string() });
        };
        let meeting_id: Option<String> = row.try_get("meeting_id")?;
        let source_key = meeting_id.unwrap_or_else(|| id.as_str().to_string());

        // The source's stored chunk vectors; a not-yet-chunked recording falls
        // back to its legacy whole-recording vector (same precedence as
        // `vector_ranking`).
        let mut rows = sqlx::query(
            "SELECT vector FROM embedding_chunks WHERE recording_id = ? ORDER BY chunk_index",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            rows = sqlx::query("SELECT vector FROM embeddings WHERE id = ?")
                .bind(id.as_str())
                .fetch_all(&self.pool)
                .await?;
        }

        // Component-wise mean of the source vectors, skipping any corrupt or
        // odd-dimension blob (same guards as the search paths), then
        // L2-renormalize — cosine is a plain dot product over unit vectors, and
        // a mean of unit vectors is shorter than unit.
        let mut mean: Vec<f32> = Vec::new();
        let mut count = 0usize;
        for row in rows {
            let bytes: Vec<u8> = row.try_get("vector")?;
            if !bytes.len().is_multiple_of(4) {
                tracing::warn!(id = %id, len = bytes.len(), "skipping source embedding: not 4-byte aligned");
                continue;
            }
            let vec: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|c| {
                    f32::from_le_bytes(
                        c.try_into()
                            .expect("chunks_exact(4) yields exactly 4 bytes"),
                    )
                })
                .collect();
            if mean.is_empty() {
                mean = vec;
                count = 1;
            } else if vec.len() == mean.len() {
                for (m, v) in mean.iter_mut().zip(&vec) {
                    *m += v;
                }
                count += 1;
            } else {
                tracing::warn!(id = %id, dim = vec.len(), mean_dim = mean.len(), "skipping source embedding: dimension mismatch");
            }
        }
        if count == 0 {
            return Err(crate::error::Error::Internal(format!(
                "recording {id} isn't indexed for semantic search yet — re-embed the library or wait for the pipeline to index it"
            )));
        }
        for m in &mut mean {
            *m /= count as f32;
        }
        crate::embed::l2_normalize(&mut mean);

        // Score every OTHER recording by its best chunk against the centroid.
        let ranking = self.vector_ranking(&mean).await?;
        let mut results: Vec<(RecordingId, f32)> = Vec::new();
        for (key, rec_id, cosine) in ranking {
            if results.len() >= limit {
                break;
            }
            if key == source_key {
                continue; // never recommend the source (or its meeting sibling)
            }
            let relevance = crate::fusion::calibrate_cosine(cosine);
            if relevance < min_relevance {
                continue;
            }
            results.push((rec_id, relevance));
        }
        Ok(results)
    }
}
