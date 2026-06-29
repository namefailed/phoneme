//! Embedding cache internals plus embedding storage and hybrid search.

use super::*;

/// RRF weights for the two fused retrievers, in `[vector, lexical]` order. The
/// semantic list is weighted slightly higher — paraphrase recall is the point —
/// with the lexical list as the complementary safety net. Shared by every fusion
/// caller ([`Catalog::hybrid_search`], the search bar, and [`Catalog::retrieve_context`],
/// the Ask RAG) so the two can't drift.
const HYBRID_RRF_WEIGHTS: [f32; 2] = [1.0, 0.85];

/// Small relevance floor for a lexical-only hit so it surfaces honestly rather
/// than reading "0% relevant" despite being an exact-term match. Shared by the
/// search bar and Ask so both floor identically.
const LEXICAL_ONLY_RELEVANCE: f32 = 0.30;

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
        // Keep the ANN index in step through the SAME choke point (no-op unless
        // ANN is enabled): remove the recording's old keys and add the new chunk
        // vectors. Routing both structures through one call site keeps them
        // coherent and reuses the proven race discipline of the cache patch.
        self.sync_recording_to_ann(id, vectors).await;
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
        // The ANN key map is derived from the chunks just deleted — clear it in
        // the same transaction so a later re-embed allocates fresh keys rather
        // than reusing stale ones. Purely additive: a no-op on a library that
        // never enabled ANN (the table is just empty).
        sqlx::query("DELETE FROM ann_keys")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        // Whole library wiped for a re-embed — drop the snapshot.
        self.invalidate_embedding_cache();
        // The chunk vectors are gone, so the ANN index is stale: drop it + the
        // sidecar (no-op unless the feature is compiled). A later re-embed
        // re-allocates keys and the daemon rebuilds the index from SQLite.
        self.clear_ann_index();
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
    /// LEGACY / not the production search. This scans ONLY the `embeddings`
    /// (whole-recording) corpus, so on a chunk-embedded library — where the
    /// backfill drains `embeddings` and per-chunk vectors carry the recall — it
    /// returns near-nothing. The user-facing `SemanticSearch` request routes to
    /// [`Self::hybrid_search`] (vector + lexical fusion over chunk vectors); the
    /// only callers left here are tests. Do NOT wire a user-facing search to this.
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

        // ANN candidate narrowing (only when the feature is compiled, the flag is
        // on, a warm index exists, and its dimension matches). Returns the set of
        // chunk-bearing recording ids whose vectors the scan should consider, plus
        // the (few) legacy-only recordings that have no chunks yet. `None` means
        // "no ANN this query" → the full brute-force scan below, verbatim.
        let ann_candidate_ids = self.ann_candidate_recording_ids(&query, dim).await;

        // Best-chunk cosine is CPU-bound — up to MAX_CACHED_VECTORS dot products
        // on the brute-force path — so run it on the blocking pool rather than
        // inline on the async executor, where a large library would stall IPC
        // reads / audio streaming between await points.
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

            // Per-chunk vectors (the primary, high-recall path). On the ANN path
            // only the candidate recordings' chunks are scored; on the brute-force
            // path every chunk is. The exact scoring math, dimension guard, and
            // meeting-dedupe are identical either way — ANN changes *which*
            // candidates are scored, never *how* — so the returned scores stay
            // bit-identical to brute force.
            let mut have_chunks: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            for cv in &corpus.chunks {
                have_chunks.insert(cv.id.as_str());
                if let Some(ids) = &ann_candidate_ids {
                    if !ids.contains(cv.id.as_str()) {
                        continue; // not in the ANN candidate set this query
                    }
                }
                consider(cv);
            }

            // Legacy whole-recording vectors, only for recordings not yet chunked,
            // so the library stays searchable while the backfill runs. These are
            // ALWAYS scanned (even on the ANN path): they're few — the backfill
            // drains them — and scanning them preserves the "searchable during
            // migration" guarantee for recordings the index doesn't cover yet.
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
        let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let like = format!("%{escaped}%");
        let rows = sqlx::query(
            "SELECT r.id AS id, r.meeting_id AS meeting_id \
             FROM recordings r \
             JOIN recording_tags rt ON rt.recording_id = r.id \
             JOIN tags t ON t.id = rt.tag_id \
             WHERE t.name LIKE ? ESCAPE '\\' \
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

    /// The shared core of [`Self::hybrid_search`] and [`Self::retrieve_context`]:
    /// run the two (optionally three) retrievers, fuse them with RRF, and return
    /// the surviving results in fused order as
    /// `(dedupe_key, RecordingId, display_relevance, matched_lexically)`.
    ///
    /// `display_relevance` is the calibrated best-chunk cosine, floored to
    /// [`LEXICAL_ONLY_RELEVANCE`] for a lexical hit; a semantic-only hit below
    /// `min_relevance` is dropped. When `filter` is `Some`, results are restricted
    /// to its in-scope dedupe keys (predicate fields only — see
    /// [`Self::hybrid_search`]'s doc) before this returns, so the caller's
    /// `limit`/`top_k` cut keeps the top in-scope results. `include_tags` folds
    /// tag-name matches into the lexical side (the search bar wants this; Ask
    /// deliberately doesn't — see [`Self::retrieve_context`]).
    ///
    /// The caller layers what it needs on top: [`Self::hybrid_search`] just maps to
    /// `(id, relevance)` + truncates; [`Self::retrieve_context`] recovers the
    /// best-matching chunk per surviving result for citation granularity.
    async fn fuse_hybrid(
        &self,
        query: &str,
        query_vec: &[f32],
        min_relevance: f32,
        filter: Option<&ListFilter>,
        include_tags: bool,
    ) -> Result<Vec<(String, RecordingId, f32, bool)>> {
        // When a filter is given, pre-compute the in-scope dedupe keys so the fused
        // ranking can be restricted to them. Built from the same `list` query the
        // Library uses (predicate fields only — query and pagination dropped), then
        // mapped to dedupe keys so a meeting passes if either of its tracks matches.
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
        // Ask omits this (`include_tags == false`): grounding an answer on a
        // recording whose tag *name* contains a query word is noise.
        if include_tags {
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
        let fused = crate::fusion::reciprocal_rank_fusion(
            &[&vec_keys[..], &lex_keys[..]],
            Some(&HYBRID_RRF_WEIGHTS),
        );

        let mut out: Vec<(String, RecordingId, f32, bool)> = Vec::new();
        for (key, _fused_score) in fused {
            // Restrict to the in-scope candidate set when a filter was given.
            // Applied here — after ranking, before the caller's cut — so the top
            // in-scope results survive rather than the top overall.
            if let Some(allowed) = &allowed_keys {
                if !allowed.contains(&key) {
                    continue;
                }
            }
            let Some(rec_id) = rec_id_by_key.get(&key).cloned() else {
                continue;
            };
            let matched_lexically = lexical_keys.contains(&key);
            let relevance = match cosine_by_key.get(&key) {
                Some(c) => crate::fusion::calibrate_cosine(*c),
                None => 0.0,
            };
            // A lexical hit is kept regardless of its (possibly weak) cosine; a
            // semantic-only hit must clear the relevance floor.
            let display = if matched_lexically {
                relevance.max(LEXICAL_ONLY_RELEVANCE)
            } else {
                relevance
            };
            if !matched_lexically && display < min_relevance {
                continue;
            }
            out.push((key, rec_id, display, matched_lexically));
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
        // The shared retrieve + fuse + floor + S3-scope core, with tag-name folding
        // (the search bar wants it). Map to `(id, relevance)` and truncate.
        let fused = self
            .fuse_hybrid(query, query_vec, min_relevance, filter, true)
            .await?;
        let mut results: Vec<(RecordingId, f32)> = fused
            .into_iter()
            .map(|(_key, rec_id, display, _matched_lexically)| (rec_id, display))
            .collect();
        results.truncate(limit);
        Ok(results)
    }

    /// Retrieve the top grounding chunks for an Ask-my-archive question (local
    /// RAG), each carrying enough to ground a prompt and to map an answer
    /// citation back to its source recording + chunk.
    ///
    /// This rides the exact same hybrid retrieval [`Self::hybrid_search`] uses —
    /// `vector_ranking` + `lexical_ranking` fused with the identical RRF call and
    /// weights, the same meeting-stable dedupe keys, the same `LEXICAL_ONLY` floor
    /// — so an answer is grounded on the same evidence the search bar would
    /// surface, with one addition: it recovers the single best-matching chunk per
    /// surviving result (which `hybrid_search` discards) so a `[n]` citation can
    /// point at the exact passage.
    ///
    /// Deliberate divergence from `hybrid_search`: tag-name folding
    /// (`tag_ranking`) is **omitted**. Ask is a meaning-question, and grounding an
    /// answer on a recording that merely carries a tag whose *name* contains a
    /// query word is noise. The consequence is intended: for a tag-name query
    /// Ask's candidate set is a strict subset of the search bar's.
    ///
    /// Chunk recovery, per surviving result: re-read its stored chunk vectors
    /// (`SELECT vector FROM embedding_chunks … ORDER BY chunk_index`, the same
    /// query `more_like_this` uses), score each against `query_vec`, take the
    /// argmax row's ordinal as the chunk index, and re-derive that chunk's text
    /// from the live transcript via the pure [`crate::chunk::chunk_transcript`].
    /// Edge cases the `text` invariant (never empty) forces:
    /// - transcript edited shorter than the stored vectors → clamp the index to
    ///   the last chunk; if still empty, fall back to a transcript prefix;
    /// - a lexical-only / legacy-only hit (no `embedding_chunks` rows to argmax
    ///   over) → `chunk_index = -1`, `is_lexical = true`, snippet is a transcript
    ///   prefix;
    /// - no transcript at all (audio-only / retention-reclaimed) → drop the
    ///   result; it can't ground anything.
    ///
    /// `filter` scopes the candidate set with the *same* `allowed_keys`
    /// restriction as `hybrid_search` (a meeting passes when either track is in
    /// scope), applied after ranking and before the `top_k` cut. `relevance` is
    /// the calibrated best-chunk cosine, identical to the search bar's chip value.
    pub async fn retrieve_context(
        &self,
        query: &str,
        query_vec: &[f32],
        top_k: usize,
        min_relevance: f32,
        filter: Option<&ListFilter>,
    ) -> Result<Vec<RetrievedChunk>> {
        // The exact same retrieve + fuse + floor + scope core the search bar uses,
        // minus tag-name folding (`include_tags == false` — grounding on a tag
        // *name* match is noise; see the doc comment). Returns surviving results in
        // fused order as `(key, rec_id, display, matched_lexically)`; we layer
        // chunk recovery on top. `matched_lexically` gates the floor (already
        // applied inside `fuse_hybrid`) but is NOT the emitted `is_lexical` flag —
        // that's decided below by whether a per-chunk vector was actually recovered
        // for the citation (a recording can match BOTH retrievers, and when it has
        // a real chunk to cite it is a vector hit, not a lexical-only one).
        let fused = self
            .fuse_hybrid(query, query_vec, min_relevance, filter, false)
            .await?;

        let mut out: Vec<RetrievedChunk> = Vec::new();
        for (_key, rec_id, display, _matched_lexically) in fused {
            if out.len() >= top_k {
                break;
            }

            // Recover the recording's live transcript; an audio-only /
            // retention-reclaimed row can't ground anything, so it is dropped.
            let Some(recording) = self.get(&rec_id).await? else {
                continue;
            };
            let Some(transcript) = recording
                .transcript
                .as_deref()
                .filter(|t| !t.trim().is_empty())
            else {
                continue;
            };
            let meeting_id = recording.meeting_id.clone();
            let chunks = crate::chunk::chunk_transcript(transcript);

            // Argmax the recording's stored chunk vectors against the query to
            // find which chunk matched (citation granularity). A lexical-only /
            // legacy-only hit has no chunk vectors to argmax over.
            let chunk_rows = sqlx::query(
                "SELECT vector FROM embedding_chunks WHERE recording_id = ? ORDER BY chunk_index",
            )
            .bind(rec_id.as_str())
            .fetch_all(&self.pool)
            .await?;

            let mut best_chunk: Option<usize> = None;
            let mut best_score = f32::NEG_INFINITY;
            for (ordinal, row) in chunk_rows.iter().enumerate() {
                let bytes: Vec<u8> = row.try_get("vector")?;
                if !bytes.len().is_multiple_of(4) {
                    continue; // corrupt blob — same guard as the scan paths
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
                if vec.len() != query_vec.len() {
                    continue; // dimension mismatch — skip, same as the scan paths
                }
                let score = crate::embed::Embedder::cosine_similarity(query_vec, &vec);
                if score > best_score {
                    best_score = score;
                    best_chunk = Some(ordinal);
                }
            }

            // Derive the snippet + index. `text` must never be empty.
            let (chunk_index, text) = match best_chunk {
                // A per-chunk vector won the argmax: cite that chunk, clamping the
                // index to the live transcript's chunk count (a transcript edited
                // shorter than the stored vectors), and falling back to a prefix
                // if the clamped chunk is somehow empty.
                Some(ordinal) if !chunks.is_empty() => {
                    let idx = ordinal.min(chunks.len() - 1);
                    let chunk_text = chunks[idx].trim();
                    if chunk_text.is_empty() {
                        (-1, transcript_prefix(transcript))
                    } else {
                        (idx as i64, chunk_text.to_string())
                    }
                }
                // No usable per-chunk vector (lexical-only / legacy-only, or every
                // blob was corrupt/dim-mismatched): cite a transcript prefix.
                _ => (-1, transcript_prefix(transcript)),
            };

            // The emitted flag tracks the citation, not the retriever set: a
            // recovered per-chunk vector (`chunk_index >= 0`) is a vector hit even
            // when the recording also matched lexically; only a hit with no usable
            // per-chunk vector (`chunk_index == -1`) is reported lexical-only. This
            // matches the `RetrievedChunk::is_lexical` contract (it drives the
            // snippet fallback + the lexical floor, both of which apply only when
            // there was no chunk to cite).
            let is_lexical = chunk_index < 0;

            out.push(RetrievedChunk {
                recording_id: rec_id,
                meeting_id,
                chunk_index,
                text,
                relevance: display,
                is_lexical,
            });
        }

        Ok(out)
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

// ── ANN (approximate nearest-neighbour) index lifecycle + retrieval ──────────
//
// Every method here is a no-op (or returns `None`) when the `ann-usearch`
// feature is off OR `ann_config.enabled` is false, so a catalog that never
// turned ANN on behaves exactly as before. The index is a disposable derived
// cache over the `embedding_chunks` BLOBs; the `ann_keys` table maps the usearch
// `u64` keys back to `(recording_id, chunk_index)`. Brute force is the always-
// present fallback: any ANN error logs a warn, drops the index/sidecar, and the
// caller falls through to the cosine scan — never an error to the user.
impl Catalog {
    /// Snapshot of the ANN tuning config (cheap clone under a short read lock).
    pub(crate) fn ann_config_snapshot(&self) -> AnnConfig {
        self.ann_config
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|poison| poison.into_inner().clone())
    }

    /// Whether the ANN path is active right now: the feature is compiled AND the
    /// runtime flag is on. Does not check whether a warm index exists. `pub` so
    /// the daemon can gate its startup load/rebuild + log on it.
    pub fn ann_enabled(&self) -> bool {
        ann::feature_compiled() && self.ann_config_snapshot().enabled
    }

    /// Set the ANN tuning config (the daemon calls this after `open` from the
    /// loaded `Config`). Turning ANN off drops any warm index + sidecar so a
    /// later re-enable rebuilds cleanly; turning it on does not build here —
    /// the daemon background-builds so startup never blocks. Safe to call on a
    /// default build: with the feature off, `enabled` has no runtime effect.
    pub fn set_ann_config(&self, cfg: AnnConfig) {
        let now_enabled = ann::feature_compiled() && cfg.enabled;
        {
            let mut guard = match self.ann_config.write() {
                Ok(g) => g,
                Err(poison) => poison.into_inner(),
            };
            *guard = cfg;
        }
        if !now_enabled {
            self.drop_ann_index();
        }
    }

    /// Drop the in-memory ANN index (the sidecar on disk is left as-is; callers
    /// that want it gone call [`Catalog::delete_ann_sidecar`]).
    fn drop_ann_index(&self) {
        let mut guard = match self.ann.write() {
            Ok(g) => g,
            Err(poison) => poison.into_inner(),
        };
        *guard = None;
    }

    /// Best-effort delete of the on-disk sidecar. A missing file is success.
    fn delete_ann_sidecar(&self) {
        if let Some(path) = &self.ann_sidecar {
            if path.exists() {
                if let Err(e) = std::fs::remove_file(path) {
                    tracing::warn!(path = %path.display(), error = %e, "ann index: failed to delete sidecar");
                }
            }
        }
    }

    /// The ANN-narrowed candidate recording-id set for a query, or `None` to fall
    /// back to the full brute-force scan. `None` is returned whenever ANN is off,
    /// no warm index exists, the index dimension doesn't match the query, or the
    /// search errors — so the caller's brute-force path is always the guaranteed
    /// fallback. On success the set holds the recording ids whose chunks the
    /// re-score should consider (resolved from the `ann_keys` table); the caller
    /// still scans legacy-only recordings unconditionally.
    async fn ann_candidate_recording_ids(
        &self,
        query: &[f32],
        dim: usize,
    ) -> Option<std::collections::HashSet<String>> {
        if !self.ann_enabled() {
            return None;
        }
        let cfg = self.ann_config_snapshot();
        // Fetch k = limit*oversample neighbours. `vector_ranking` has no `limit`
        // of its own (the caller truncates after fusion), so oversample off a
        // generous default top-k: enough candidates to absorb the meeting-dedupe
        // / max-sim collapse while staying far below a full scan. Clamp to ≥1.
        let oversample = cfg.oversample.max(1);
        const ANN_BASE_K: usize = 200;
        let k = ANN_BASE_K.saturating_mul(oversample);

        // Search under a read lock, then resolve keys outside it. A dimension
        // mismatch or an empty index means "no usable ANN" → brute force.
        let hits: Vec<(u64, f32)> = {
            let guard = self.ann.read().ok()?;
            let index = guard.as_ref()?;
            if index.dim() != dim || index.is_empty() {
                return None;
            }
            match index.search(query, k) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(error = %e, "ann index: search failed; falling back to brute force");
                    return None;
                }
            }
        };
        if hits.is_empty() {
            // A healthy index that returns nothing is a legitimate "no neighbours"
            // — but to stay safe (never silently drop results that brute force
            // would find), treat it as a fallback rather than an empty result.
            return None;
        }

        // Resolve usearch keys → recording ids via ann_keys. Keys not found (a
        // race with a concurrent delete) are simply skipped.
        let keys: Vec<i64> = hits.iter().map(|(k, _)| *k as i64).collect();
        let placeholders = vec!["?"; keys.len()].join(",");
        let sql =
            format!("SELECT DISTINCT recording_id FROM ann_keys WHERE key IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for k in &keys {
            q = q.bind(k);
        }
        match q.fetch_all(&self.pool).await {
            // Hits that resolve to at least one recording → narrow to that set.
            Ok(ids) if !ids.is_empty() => Some(ids.into_iter().collect()),
            // Hits that resolve to ZERO rows (every key was a dead node, or
            // ann_keys drifted from the graph) must fall back, not return an empty
            // set — an empty set would make the scan skip every chunk and silently
            // serve near-empty semantic results. Mirrors the `hits.is_empty()`
            // guard above: a candidate set we can't trust → brute force.
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(error = %e, "ann index: key resolution failed; falling back to brute force");
                None
            }
        }
    }

    /// Allocate (or reuse) `ann_keys` rows for a recording's chunks and return
    /// the `key`s in chunk order. Idempotent: re-embedding reuses the same keys
    /// for unchanged `(recording_id, chunk_index)` pairs via the UNIQUE upsert,
    /// and prunes any rows past the new chunk count so a shrunk recording doesn't
    /// leave dangling keys.
    async fn allocate_ann_keys(&self, id: &RecordingId, chunk_count: usize) -> Result<Vec<u64>> {
        let mut tx = self.pool.begin().await?;
        // Drop rows for chunk indices that no longer exist (recording shrank).
        sqlx::query("DELETE FROM ann_keys WHERE recording_id = ? AND chunk_index >= ?")
            .bind(id.as_str())
            .bind(chunk_count as i64)
            .execute(&mut *tx)
            .await?;
        if chunk_count == 0 {
            tx.commit().await?;
            return Ok(Vec::new());
        }
        // One multi-row INSERT OR IGNORE instead of a statement per chunk: a chunk
        // that already has a key keeps it (the UNIQUE upsert), a missing one gets
        // allocated. This collapses 2*chunk_count round-trips (insert+select each)
        // into two statements, which matters on a full reindex that calls this per
        // recording across the whole library.
        let placeholders = vec!["(?, ?)"; chunk_count].join(", ");
        let insert = format!(
            "INSERT OR IGNORE INTO ann_keys (recording_id, chunk_index) VALUES {placeholders}"
        );
        let mut q = sqlx::query(&insert);
        for idx in 0..chunk_count {
            q = q.bind(id.as_str()).bind(idx as i64);
        }
        q.execute(&mut *tx).await?;
        // Read every key back in one SELECT. After the tail prune + insert above,
        // exactly chunk_index 0..chunk_count exist for this recording; map each
        // row's key into its chunk position so the returned Vec is in chunk order.
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT key, chunk_index FROM ann_keys WHERE recording_id = ? ORDER BY chunk_index",
        )
        .bind(id.as_str())
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut keys = vec![0u64; chunk_count];
        for (key, chunk_index) in rows {
            if let Some(slot) = keys.get_mut(chunk_index as usize) {
                *slot = key as u64;
            }
        }
        Ok(keys)
    }

    /// The recording's ANN keys, for the delete path to drop from the index
    /// before the FK cascade removes the `ann_keys` rows. Returns empty (no DB
    /// hit) unless ANN is enabled, and swallows a read error into empty — a
    /// missed remove only leaves a dead node the next rebuild reclaims, never an
    /// error to the user. `pub(crate)` so the sibling `recordings` module can
    /// call it.
    pub(crate) async fn recording_ann_keys_for_delete(&self, id: &RecordingId) -> Vec<u64> {
        if !self.ann_enabled() {
            return Vec::new();
        }
        self.recording_ann_keys(id).await.unwrap_or_default()
    }

    /// The current `ann_keys` for a recording, `(key, chunk_index)` ascending —
    /// used to remove a recording's old vectors from the index before re-adding
    /// or on delete.
    async fn recording_ann_keys(&self, id: &RecordingId) -> Result<Vec<u64>> {
        let rows: Vec<i64> = sqlx::query_scalar(
            "SELECT key FROM ann_keys WHERE recording_id = ? ORDER BY chunk_index",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|k| k as u64).collect())
    }

    /// Keep the ANN index in step with one recording's chunk vectors: remove its
    /// old keys, allocate keys for the new chunks, and `add` them. Called from
    /// `upsert_chunk_embeddings` after the DB write + cache patch, reusing the
    /// same single choke point so the index and the warm cache stay coherent.
    ///
    /// A no-op unless ANN is enabled. Any error logs a warn and drops the index
    /// (a rebuild from SQLite then heals it) — a stale ANN must never serve.
    pub(crate) async fn sync_recording_to_ann(&self, id: &RecordingId, vectors: &[Vec<f32>]) {
        if !self.ann_enabled() {
            return;
        }
        // Old keys to remove from the graph (before they're reallocated/pruned).
        let old_keys = self.recording_ann_keys(id).await.unwrap_or_default();
        // Allocate stable keys for the new chunk set (also prunes shrunk tail).
        let new_keys = match self.allocate_ann_keys(id, vectors.len()).await {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!(id = %id.as_str(), error = %e, "ann index: key allocation failed; dropping index");
                self.drop_ann_index();
                return;
            }
        };
        let guard = match self.ann.read() {
            Ok(g) => g,
            Err(poison) => poison.into_inner(),
        };
        let Some(index) = guard.as_ref() else {
            // No warm index yet (the daemon hasn't built it). The keys are now in
            // ann_keys, so the eventual build/rebuild picks them up — nothing to do.
            return;
        };
        // Remove any old keys that won't be reused, then add the current vectors.
        let reused: std::collections::HashSet<u64> = new_keys.iter().copied().collect();
        for k in old_keys {
            if !reused.contains(&k) {
                if let Err(e) = index.remove(k) {
                    tracing::warn!(key = k, error = %e, "ann index: remove during re-embed failed");
                }
            }
        }
        for (vec, key) in vectors.iter().zip(&new_keys) {
            // A re-embed reuses the key, so remove the stale vector first (add
            // alone would error or duplicate). A fresh key isn't present, so the
            // remove is a harmless no-op.
            let _ = index.remove(*key);
            if let Err(e) = index.add(*key, vec) {
                tracing::warn!(id = %id.as_str(), key = *key, error = %e, "ann index: add failed; dropping index");
                drop(guard);
                self.drop_ann_index();
                return;
            }
        }
    }

    /// Remove a recording's vectors from the ANN index (its `ann_keys` rows are
    /// dropped by the FK cascade when the recording row goes). Called from the
    /// recording-delete path alongside `patch_recording_in_cache`. A no-op unless
    /// ANN is enabled. Capture the keys BEFORE the cascade removes them.
    pub(crate) async fn remove_recording_from_ann_keys(&self, keys: &[u64]) {
        if !self.ann_enabled() || keys.is_empty() {
            return;
        }
        let guard = match self.ann.read() {
            Ok(g) => g,
            Err(poison) => poison.into_inner(),
        };
        let Some(index) = guard.as_ref() else {
            return;
        };
        for &k in keys {
            if let Err(e) = index.remove(k) {
                tracing::warn!(key = k, error = %e, "ann index: remove on delete failed");
            }
        }
    }

    /// Drop the in-memory index and delete the sidecar — the ANN twin of
    /// `clear_all_embeddings` / `clear_all_recordings`. The `ann_keys` rows are
    /// taken by the same cascade/DELETE that clears the embedding tables.
    pub(crate) fn clear_ann_index(&self) {
        if !ann::feature_compiled() {
            return;
        }
        self.drop_ann_index();
        self.delete_ann_sidecar();
    }

    /// All chunk vectors with their ANN keys, for a full index (re)build. Decodes
    /// the `embedding_chunks` BLOBs joined to `ann_keys`; a chunk missing a key
    /// row (it predates the table) gets one allocated. Skips corrupt blobs (the
    /// same guard the scan uses). Returns `(dim, pairs)`; `dim` is the first
    /// good vector's length, or `None` when there's nothing to index.
    async fn collect_ann_build_pairs(&self) -> Result<Option<(usize, Vec<(u64, Vec<f32>)>)>> {
        // Ensure every chunk has a key row first (covers a library embedded
        // before ANN was enabled). Group chunk counts per recording, then
        // allocate keys for each.
        let counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT recording_id, COUNT(*) AS n FROM embedding_chunks GROUP BY recording_id",
        )
        .fetch_all(&self.pool)
        .await?;
        for (rid, n) in &counts {
            if let Some(id) = RecordingId::parse(rid.clone()) {
                self.allocate_ann_keys(&id, *n as usize).await?;
            }
        }

        // Join chunks to their keys and decode.
        let rows = sqlx::query(
            "SELECT ak.key AS key, ec.vector AS vector \
             FROM embedding_chunks ec \
             JOIN ann_keys ak ON ak.recording_id = ec.recording_id \
                             AND ak.chunk_index = ec.chunk_index",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut dim: Option<usize> = None;
        let mut pairs: Vec<(u64, Vec<f32>)> = Vec::with_capacity(rows.len());
        for row in rows {
            let key: i64 = row.try_get("key")?;
            let bytes: Vec<u8> = row.try_get("vector")?;
            if !bytes.len().is_multiple_of(4) {
                continue; // corrupt blob, skip (matches the scan's guard)
            }
            let vec: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().expect("chunks_exact(4) yields 4 bytes")))
                .collect();
            if vec.is_empty() {
                continue;
            }
            match dim {
                None => dim = Some(vec.len()),
                Some(d) if d != vec.len() => continue, // mixed dims → skip the odd one
                _ => {}
            }
            pairs.push((key as u64, vec));
        }
        Ok(dim.map(|d| (d, pairs)))
    }

    /// Build (or rebuild) the ANN index from SQLite and swap it in warm. Drives
    /// the daemon's background build after the embedding backfill drains, and the
    /// CLI `phoneme reindex`. CPU-heavy (HNSW build), so the daemon runs it under
    /// `spawn_blocking`; the SQLite reads here are async. A no-op unless ANN is
    /// enabled. On any error the index is left `None` (brute force) — never fatal.
    pub async fn rebuild_ann_index(&self) -> Result<()> {
        if !self.ann_enabled() {
            return Ok(());
        }
        let cfg = self.ann_config_snapshot();
        let Some(sidecar) = self.ann_sidecar.clone() else {
            tracing::debug!("ann index: no on-disk sidecar (in-memory db); skipping build");
            return Ok(());
        };

        // Serialize the read-snapshot → build → swap window against concurrent
        // incremental adds (`sync_recording_to_ann`). An add commits its DB row,
        // patches the warm cache (bumping `embedding_cache_gen` under the cache
        // write lock), THEN touches the ANN graph — so a bump is the canonical
        // "an add happened" signal. If a recording is added while we hold a stale
        // read snapshot, our freshly-built index would silently miss it: when its
        // `sync_recording_to_ann` ran, the index was either still `None` (so the
        // add only landed in SQLite, which our snapshot may predate) or it mutated
        // the previous index we're about to overwrite. We re-check the generation
        // under the `ann.write()` lock at swap time and replay if it advanced, so
        // no add is lost. Bounded so a steady write stream can't starve the build;
        // once we swap the warm index in, subsequent adds re-sync into it directly.
        const MAX_REBUILD_REPLAYS: u32 = 8;
        for _ in 0..MAX_REBUILD_REPLAYS {
            let gen_at_snapshot = self.embedding_cache_gen.load(Ordering::Acquire);
            let Some((dim, pairs)) = self.collect_ann_build_pairs().await? else {
                // Nothing to index yet — drop any stale index so search falls back.
                self.drop_ann_index();
                return Ok(());
            };
            let index = match ann::AnnIndex::build_from_pairs(sidecar.clone(), dim, &pairs, &cfg) {
                Ok(index) => index,
                Err(e) => {
                    tracing::warn!(error = %e, "ann index: build failed; staying on brute force");
                    self.drop_ann_index();
                    self.delete_ann_sidecar();
                    return Ok(());
                }
            };

            // Swap under the write lock and re-check the generation while holding
            // it. `sync_recording_to_ann` takes `ann.read()` only AFTER its cache
            // patch bumps the generation, so observing the same generation here —
            // with the write lock held — means no add committed during our window
            // and the built index is complete.
            let mut guard = match self.ann.write() {
                Ok(g) => g,
                Err(poison) => poison.into_inner(),
            };
            if self.embedding_cache_gen.load(Ordering::Acquire) == gen_at_snapshot {
                if let Err(e) = index.save() {
                    tracing::warn!(error = %e, "ann index: save after build failed (index still usable in memory)");
                }
                *guard = Some(index);
                tracing::info!(vectors = pairs.len(), dim, "ann index: built");
                return Ok(());
            }
            // An add raced the build. Drop the lock and rebuild from a fresh
            // snapshot so the new recording is included.
            drop(guard);
            tracing::debug!("ann index: incremental add raced the build; replaying");
        }

        // Exhausted the bounded replays under sustained writes. Build one final
        // index from the latest snapshot and swap it in: it's at worst missing the
        // very last in-flight add, which the now-warm index picks up on its next
        // `sync_recording_to_ann` (the index is no longer `None`). Never leave the
        // search on a stale or empty graph indefinitely.
        let Some((dim, pairs)) = self.collect_ann_build_pairs().await? else {
            self.drop_ann_index();
            return Ok(());
        };
        match ann::AnnIndex::build_from_pairs(sidecar, dim, &pairs, &cfg) {
            Ok(index) => {
                if let Err(e) = index.save() {
                    tracing::warn!(error = %e, "ann index: save after build failed (index still usable in memory)");
                }
                let mut guard = match self.ann.write() {
                    Ok(g) => g,
                    Err(poison) => poison.into_inner(),
                };
                *guard = Some(index);
                tracing::info!(
                    vectors = pairs.len(),
                    dim,
                    "ann index: built (after replay cap)"
                );
                Ok(())
            }
            Err(e) => {
                tracing::warn!(error = %e, "ann index: build failed; staying on brute force");
                self.drop_ann_index();
                self.delete_ann_sidecar();
                Ok(())
            }
        }
    }

    /// Load the index from its sidecar if it's healthy, else rebuild from SQLite.
    /// The daemon calls this once at startup (under `spawn_blocking` for the
    /// build path) so a warm-start reuses the persisted graph and a cold/stale
    /// one heals. A no-op unless ANN is enabled.
    pub async fn load_or_rebuild_ann_index(&self) -> Result<()> {
        if !self.ann_enabled() {
            return Ok(());
        }
        let cfg = self.ann_config_snapshot();
        let Some(sidecar) = self.ann_sidecar.clone() else {
            return Ok(());
        };
        // Dim + expected count from SQLite (the source of truth) to verify the
        // sidecar against. `expected_count` must reflect ONLY the actually-
        // indexable vectors — those `collect_ann_build_pairs` would keep — not
        // every chunk row. A single corrupt or off-dimension blob is skipped by
        // the build, so counting all rows would make the count check fail forever
        // (rebuild every startup). Count only well-formed blobs at this dim, which
        // is exactly the set the index was built from.
        let dim = self.ann_dim_from_sqlite().await;
        if let (true, Some(dim)) = (sidecar.exists(), dim) {
            let expected_count = self.ann_indexable_count(dim).await;
            match ann::AnnIndex::load_verified(sidecar.clone(), dim, expected_count, &cfg) {
                Ok(index) => {
                    let mut guard = match self.ann.write() {
                        Ok(g) => g,
                        Err(poison) => poison.into_inner(),
                    };
                    *guard = Some(index);
                    tracing::info!(
                        vectors = expected_count,
                        dim,
                        "ann index: loaded from sidecar"
                    );
                    return Ok(());
                }
                Err(e) => {
                    tracing::info!(error = %e, "ann index: sidecar unusable; rebuilding from SQLite");
                }
            }
        }
        self.rebuild_ann_index().await
    }

    /// How many chunk vectors are actually indexable at `dim` — the count
    /// `collect_ann_build_pairs` would keep. A vector is kept iff its blob is
    /// well-formed and decodes to exactly `dim` floats, i.e. its byte length is
    /// `dim * 4` (which is non-empty and a multiple of 4 for any `dim >= 1`).
    /// Joining to `ann_keys` mirrors the build's join so a chunk without a key row
    /// (not yet allocated) isn't counted as a sidecar vector. This is the count
    /// `load_verified` checks the persisted graph against, so a corrupt or
    /// off-dimension blob can't make a healthy sidecar fail its count check.
    async fn ann_indexable_count(&self, dim: usize) -> usize {
        let want_bytes = (dim * 4) as i64;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ann_keys ak JOIN embedding_chunks ec \
                ON ak.recording_id = ec.recording_id AND ak.chunk_index = ec.chunk_index \
             WHERE LENGTH(ec.vector) = ?",
        )
        .bind(want_bytes)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        count as usize
    }

    /// The embedding dimension implied by the stored chunk vectors — the MODAL
    /// (most common) blob length, not an arbitrary row — or `None` when there are
    /// no well-formed chunks. A mixed-dimension library (mid model-change, before a
    /// `clear_all_embeddings` + re-embed) holds blobs of two lengths; picking the
    /// majority keeps this dim, `ann_indexable_count`, and the actual build in
    /// agreement, instead of an unordered `LIMIT 1` occasionally returning the
    /// minority dimension and triggering a rebuild every startup.
    async fn ann_dim_from_sqlite(&self) -> Option<usize> {
        let len: Option<i64> = sqlx::query_scalar(
            "SELECT LENGTH(vector) FROM embedding_chunks \
             GROUP BY LENGTH(vector) ORDER BY COUNT(*) DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        len.and_then(|l| {
            let l = l as usize;
            (l.is_multiple_of(4) && l != 0).then_some(l / 4)
        })
    }

    /// Persist the warm index to its sidecar if one is present. The daemon calls
    /// this on graceful shutdown so the incremental `add`s since the last build
    /// survive a restart, instead of an fsync per recording. The sidecar is
    /// disposable: a missing or stale one is rebuilt from SQLite on the next
    /// start. A no-op unless ANN is enabled and an index is warm.
    pub async fn save_ann_index(&self) {
        if !self.ann_enabled() {
            return;
        }
        let guard = match self.ann.read() {
            Ok(g) => g,
            Err(poison) => poison.into_inner(),
        };
        if let Some(index) = guard.as_ref() {
            if let Err(e) = index.save() {
                tracing::warn!(error = %e, "ann index: idle save failed");
            }
        }
    }

    /// Health snapshot for the Doctor probe: whether the feature is compiled,
    /// whether the flag is on, whether a warm index exists, its vector count, and
    /// the SQLite chunk-key count it should match. The Doctor renders
    /// "healthy / rebuilding / disabled (brute-force)" from this.
    pub async fn ann_health(&self) -> AnnHealth {
        let compiled = ann::feature_compiled();
        let enabled = self.ann_config_snapshot().enabled;
        let sqlite_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ann_keys ak JOIN embedding_chunks ec \
             ON ak.recording_id = ec.recording_id AND ak.chunk_index = ec.chunk_index",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        let index_count = self
            .ann
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|i| i.len()));
        AnnHealth {
            feature_compiled: compiled,
            enabled,
            index_loaded: index_count.is_some(),
            index_vectors: index_count.unwrap_or(0),
            sqlite_vectors: sqlite_count as usize,
        }
    }
}

/// A char-boundary-safe leading slice of a transcript, used as the snippet for a
/// lexical-only / legacy-only Ask hit that has no per-chunk vector to argmax over
/// (and as the clamp fallback when an edited transcript yields an empty chunk).
/// `text` on a [`RetrievedChunk`] must never be empty, so this always returns a
/// non-empty string for a non-empty transcript. Capped generously here; the
/// daemon re-truncates to its own per-source prompt budget.
fn transcript_prefix(transcript: &str) -> String {
    const ASK_PREFIX_CHARS: usize = 1200;
    let trimmed = transcript.trim();
    let mut end = ASK_PREFIX_CHARS.min(trimmed.len());
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    if end < trimmed.len() {
        format!("{}…", &trimmed[..end])
    } else {
        trimmed.to_string()
    }
}

/// A snapshot of the ANN index's health for the Doctor probe. See
/// [`Catalog::ann_health`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnHealth {
    /// Whether the crate was built with the `ann-usearch` feature.
    pub feature_compiled: bool,
    /// Whether `semantic_search.ann.enabled` is set.
    pub enabled: bool,
    /// Whether a warm index is loaded in memory right now.
    pub index_loaded: bool,
    /// How many vectors the warm index holds (0 when none is loaded).
    pub index_vectors: usize,
    /// How many chunk vectors SQLite holds — what a healthy index should match.
    pub sqlite_vectors: usize,
}
