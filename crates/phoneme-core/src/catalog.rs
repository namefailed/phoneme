use crate::error::Result;
use crate::id::RecordingId;
use crate::tags::Tag;
use crate::types::{ListFilter, Recording, RecordingStatus, SpeakerName, TranscriptSegment};
use chrono::{DateTime, Local};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::Path;
use std::str::FromStr;

/// SQLite-backed recordings catalog.
///
/// All methods are async (Tokio). The pool is configured for WAL mode with
/// a small connection cap suitable for desktop usage (one writer at a time).
#[derive(Debug, Clone)]
pub struct Catalog {
    pool: SqlitePool,
}

/// Sanitizes a user-provided string for use in an FTS5 MATCH query.
///
/// Extracts alphanumeric terms and joins them with `* AND ` to perform a robust
/// prefix search that won't crash SQLite on invalid syntax.
fn sanitize_fts5_query(query: &str) -> String {
    let mut terms = Vec::new();
    let mut current_term = String::new();

    for c in query.chars() {
        if c.is_alphanumeric() {
            current_term.push(c);
        } else if !current_term.is_empty() {
            terms.push(format!("{}*", current_term));
            current_term.clear();
        }
    }

    if !current_term.is_empty() {
        terms.push(format!("{}*", current_term));
    }

    terms.join(" AND ")
}

impl Catalog {
    /// Open (or create) a catalog database at `path`. Runs pending migrations.
    ///
    /// WAL configuration notes:
    /// - `journal_mode=WAL` + `synchronous=NORMAL` → ACID with crash safety,
    ///   no fsync per write.
    /// - `wal_autocheckpoint=1000` triggers an automatic checkpoint when the
    ///   WAL reaches ~1000 pages (~4 MB). Long-lived readers can still defer
    ///   the checkpoint, so `Catalog::checkpoint()` is called explicitly from
    ///   the daemon on idle to keep WAL growth bounded.
    /// - `journal_size_limit=67108864` caps the WAL at 64 MB regardless.
    pub async fn open(path: &Path) -> Result<Self> {
        let path_str = path.to_str().ok_or_else(|| {
            crate::error::Error::Internal("catalog path is not valid utf-8".into())
        })?;

        let opts = SqliteConnectOptions::from_str(path_str)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .foreign_keys(true)
            .pragma("wal_autocheckpoint", "1000")
            .pragma("journal_size_limit", "67108864");

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn insert(&self, r: &Recording) -> Result<()> {
        sqlx::query(
            "INSERT INTO recordings (
                 id, started_at, duration_ms, audio_path, transcript, model, status,
                 error_kind, error_message, hook_command, hook_exit_code, hook_duration_ms,
                 transcribed_at, hook_ran_at, notes, meeting_id, meeting_name, track, in_place,
                 cleanup_model, diarized
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(r.id.as_str())
        .bind(r.started_at.to_rfc3339())
        .bind(r.duration_ms)
        .bind(&r.audio_path)
        .bind(r.transcript.as_deref())
        .bind(r.model.as_deref())
        .bind(r.status.as_str())
        .bind(r.error_kind.as_deref())
        .bind(r.error_message.as_deref())
        .bind(r.hook_command.as_deref())
        .bind(r.hook_exit_code)
        .bind(r.hook_duration_ms)
        .bind(r.transcribed_at.map(|d| d.to_rfc3339()))
        .bind(r.hook_ran_at.map(|d| d.to_rfc3339()))
        .bind(r.notes.as_deref())
        .bind(r.meeting_id.as_deref())
        .bind(r.meeting_name.as_deref())
        .bind(r.track.as_deref())
        .bind(r.in_place)
        .bind(r.cleanup_model.as_deref())
        .bind(r.diarized)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_meeting_name(&self, meeting_id: &str, name: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE recordings SET meeting_name = ?, updated_at = datetime('now') WHERE meeting_id = ?")
            .bind(name)
            .bind(meeting_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_status(&self, id: &RecordingId, status: RecordingStatus) -> Result<()> {
        sqlx::query("UPDATE recordings SET status = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(status.as_str())
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update both status and duration in a single query.
    pub async fn update_status_and_duration(
        &self,
        id: &RecordingId,
        status: RecordingStatus,
        duration_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE recordings SET status = ?, duration_ms = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(duration_ms)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the transcript after machine transcription.
    ///
    /// `transcript` is the text to store as the live transcript — for
    /// recordings with LLM post-processing enabled this will be the LLM-
    /// cleaned text. `original_transcript` is **always** the raw Whisper
    /// output, so the "View original" feature shows the pre-LLM version even
    /// when post-processing is active. Re-transcription overwrites both
    /// columns (fresh baseline).
    pub async fn update_transcript(
        &self,
        id: &RecordingId,
        transcript: &str,
        original_transcript: &str,
        model: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET transcript = ?, original_transcript = ?, clean_transcript = ?, model = ?,
                   user_edited = 0,
                   transcribed_at = datetime('now'), updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(transcript)
        .bind(original_transcript)
        // `clean_transcript` snapshots the pipeline output (transcribed + cleaned)
        // so "View unedited transcript" can show it even after the user edits the
        // live transcript. User edits go through `update_user_transcript`, which
        // leaves this column untouched.
        .bind(transcript)
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record which post-processing LLM model ran (if any) and whether speaker
    /// diarization was applied. Called by the pipeline after transcription so
    /// the list view can surface these as columns.
    pub async fn update_processing_meta(
        &self,
        id: &RecordingId,
        cleanup_model: Option<&str>,
        diarized: bool,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET cleanup_model = ?, diarized = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(cleanup_model)
        .bind(diarized)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store (or replace) the LLM-generated summary for a recording, along with
    /// the model that produced it.
    pub async fn update_summary(
        &self,
        id: &RecordingId,
        summary: &str,
        model: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET summary = ?, summary_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(summary)
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the transcript from a manual user edit, preserving
    /// `original_transcript`/`clean_transcript` so the edit can be reverted.
    /// Sets the `user_edited` flag and — crucially — leaves `model` untouched so
    /// the "Transcript Model" column keeps showing the transcription model that
    /// actually produced the text (the "Edited" column surfaces the hand edit).
    pub async fn update_user_transcript(&self, id: &RecordingId, transcript: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET transcript = ?, user_edited = 1, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(transcript)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Replace the LLM-suggested tags awaiting approval for a recording.
    /// An empty slice clears the column (no lingering empty-array JSON).
    pub async fn set_tag_suggestions(&self, id: &RecordingId, names: &[String]) -> Result<()> {
        let json = if names.is_empty() {
            None
        } else {
            Some(serde_json::to_string(names)?)
        };
        sqlx::query(
            r#"UPDATE recordings
               SET tag_suggestions = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(json)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drop EVERY pending tag suggestion across the whole library (the
    /// Auto-Tagging settings' "Clear all suggestions" action). Returns how
    /// many recordings actually had suggestions to clear.
    pub async fn clear_all_tag_suggestions(&self) -> Result<u64> {
        let result = sqlx::query(
            r#"UPDATE recordings
               SET tag_suggestions = NULL, updated_at = datetime('now')
               WHERE tag_suggestions IS NOT NULL"#,
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Set or clear a recording's display title.
    ///
    /// Ownership rule: a user title (`title_is_auto = 0`) wins forever — an
    /// auto write (`is_auto = true` with a title) only lands while the title
    /// is still auto-owned, so the pipeline can refresh its own titles on
    /// retranscribe but can never clobber one the user typed. Explicit user
    /// writes always apply: `Some` + `is_auto = false` takes ownership;
    /// `None` clears the title AND reverts ownership to auto, so the next
    /// pipeline run generates a fresh one.
    ///
    /// Returns whether a row was actually updated (`false` = unknown id, or
    /// an auto write skipped because the user owns the title).
    pub async fn set_title(
        &self,
        id: &RecordingId,
        title: Option<&str>,
        is_auto: bool,
    ) -> Result<bool> {
        // A cleared title is always auto-owned — `None` means "no title,
        // generate one next run", never "user-owned empty title".
        let is_auto = is_auto || title.is_none();
        let result = if is_auto && title.is_some() {
            sqlx::query(
                r#"UPDATE recordings
                   SET title = ?, title_is_auto = 1, updated_at = datetime('now')
                   WHERE id = ? AND title_is_auto = 1"#,
            )
            .bind(title)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"UPDATE recordings
                   SET title = ?, title_is_auto = ?, updated_at = datetime('now')
                   WHERE id = ?"#,
            )
            .bind(title)
            .bind(is_auto)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?
        };
        Ok(result.rows_affected() > 0)
    }

    /// Set or clear the "favorite"/star flag for a recording (Favorites view).
    pub async fn set_favorite(&self, id: &RecordingId, favorite: bool) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET favorite = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(favorite as i64)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The preserved original (machine) transcript, if any. `None` for
    /// recordings transcribed before this column existed, or never transcribed.
    pub async fn get_original_transcript(&self, id: &RecordingId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT original_transcript FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(r.try_get::<Option<String>, _>("original_transcript")?),
            None => Ok(None),
        }
    }

    /// The preserved "unedited" transcript — the pipeline output (machine
    /// transcription + any LLM cleanup) before the user made hand edits. `None`
    /// for recordings transcribed before this column existed, or never
    /// transcribed.
    pub async fn get_clean_transcript(&self, id: &RecordingId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT clean_transcript FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(r.try_get::<Option<String>, _>("clean_transcript")?),
            None => Ok(None),
        }
    }

    /// Update the free-form user notes for a recording.
    ///
    /// Notes live in their own column and are completely independent of the
    /// transcript: neither machine (re-)transcription (`update_transcript`)
    /// nor user transcript edits (`update_user_transcript`) touch this column,
    /// so notes always survive those operations.
    pub async fn update_notes(&self, id: &RecordingId, notes: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET notes = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(notes)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_hook_result(
        &self,
        id: &RecordingId,
        command: &str,
        exit_code: i32,
        duration_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET hook_command = ?, hook_exit_code = ?, hook_duration_ms = ?,
                   hook_ran_at = datetime('now'), updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(command)
        .bind(exit_code)
        .bind(duration_ms)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &RecordingId) -> Result<Option<Recording>> {
        let row = sqlx::query("SELECT * FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        let mut rec = match row.map(row_to_recording).transpose()? {
            Some(r) => r,
            None => return Ok(None),
        };
        // Populate the speaker-name map so a single-recording fetch (the daemon's
        // GetRecording, which backs the detail view) can render custom names.
        // Tags are intentionally NOT loaded here — the detail view fetches those
        // separately via `tags_for`, matching prior behavior.
        rec.speaker_names = self.speaker_names_for(&rec.id).await.unwrap_or_default();
        Ok(Some(rec))
    }

    /// Upsert the semantic embedding vector for a recording.
    pub async fn upsert_embedding(&self, id: &RecordingId, vector: &[f32]) -> Result<()> {
        // Pack f32 array into little-endian bytes.
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

        Ok(())
    }

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
        Ok(())
    }

    /// Delete ALL stored embeddings — per-chunk and legacy whole-recording — so
    /// the whole library can be re-embedded with a newly-configured model. After
    /// this every recording counts as "without chunk embeddings", so the daemon's
    /// backfill re-embeds them. Vectors from a different model/dimension would
    /// otherwise be silently skipped (dimension guard) and the recording would be
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
                vec.push(f32::from_le_bytes(chunk.try_into().unwrap()));
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
    ///   vector is skipped (cosine over mismatched dimensions is meaningless and
    ///   would otherwise score on a silently-truncated prefix).
    /// - **Relevance floor:** results scoring below `min_score` are dropped, so a
    ///   vague/garbage query returns *few or no* results instead of `limit`
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
        let rows = sqlx::query(
            "SELECT e.id AS id, e.vector AS vector, r.meeting_id AS meeting_id \
             FROM embeddings e JOIN recordings r ON r.id = e.id",
        )
        .fetch_all(&self.pool)
        .await?;

        let dim = query_vec.len();
        // Best (id, score) per result key — meeting_id when present, else the
        // recording id — so a meeting contributes at most one result.
        let mut best: std::collections::HashMap<String, (RecordingId, f32)> =
            std::collections::HashMap::new();

        for row in rows {
            let id: String = row.try_get("id")?;
            let bytes: Vec<u8> = row.try_get("vector")?;
            let meeting_id: Option<String> = row.try_get("meeting_id")?;

            if !bytes.len().is_multiple_of(4) {
                tracing::warn!(id = %id, len = bytes.len(), "skipping embedding: not 4-byte aligned");
                continue;
            }
            let vec: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            if vec.len() != dim {
                tracing::warn!(id = %id, dim = vec.len(), query_dim = dim, "skipping embedding: dimension mismatch");
                continue;
            }

            let score = crate::embed::Embedder::cosine_similarity(query_vec, &vec);
            if score < min_score {
                continue;
            }
            let Some(rec_id) = RecordingId::parse(id.clone()) else {
                continue;
            };
            let key = meeting_id.unwrap_or(id);
            best.entry(key)
                .and_modify(|e| {
                    if score > e.1 {
                        *e = (rec_id.clone(), score);
                    }
                })
                .or_insert((rec_id, score));
        }

        let mut scores: Vec<(RecordingId, f32)> = best.into_values().collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(limit);
        Ok(scores)
    }

    /// Compute the per-recording **best-chunk** (max-sim) cosine ranking for a
    /// query vector, meeting-deduped.
    ///
    /// Returns `(dedupe_key, RecordingId, raw_cosine)` sorted high→low. The raw
    /// cosine is of the single best-matching chunk, which is what makes a
    /// paraphrase of one spoken idea rank on that idea instead of an averaged
    /// whole-note vector. The `dedupe_key` is the recording's `meeting_id` when
    /// it belongs to a meeting, else its own id — exposed so the fusion in
    /// [`Self::hybrid_search`] can collapse a meeting on the SAME key the lexical
    /// retriever uses, even if the two retrievers each pick a different track of
    /// that meeting as its representative (otherwise the meeting would surface
    /// twice). Recordings that only have a legacy whole-recording vector (no
    /// chunks yet, pending backfill) are folded in from the `embeddings` table so
    /// nothing becomes unsearchable during migration. Dimension-mismatched
    /// vectors are skipped (same guard as [`Self::semantic_search`]).
    async fn vector_ranking(&self, query_vec: &[f32]) -> Result<Vec<(String, RecordingId, f32)>> {
        let dim = query_vec.len();
        // best raw cosine per dedupe key (meeting_id or recording id).
        let mut best: std::collections::HashMap<String, (RecordingId, f32)> =
            std::collections::HashMap::new();

        let mut consider = |id: String, meeting_id: Option<String>, bytes: Vec<u8>| {
            if !bytes.len().is_multiple_of(4) {
                tracing::warn!(id = %id, len = bytes.len(), "skipping embedding: not 4-byte aligned");
                return;
            }
            let vec: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            if vec.len() != dim {
                tracing::warn!(id = %id, dim = vec.len(), query_dim = dim, "skipping embedding: dimension mismatch");
                return;
            }
            let score = crate::embed::Embedder::cosine_similarity(query_vec, &vec);
            let Some(rec_id) = RecordingId::parse(id.clone()) else {
                return;
            };
            let key = meeting_id.unwrap_or(id);
            best.entry(key)
                .and_modify(|e| {
                    if score > e.1 {
                        *e = (rec_id.clone(), score);
                    }
                })
                .or_insert((rec_id, score));
        };

        // Per-chunk vectors (the primary, high-recall path).
        let chunk_rows = sqlx::query(
            "SELECT ec.recording_id AS id, ec.vector AS vector, r.meeting_id AS meeting_id \
             FROM embedding_chunks ec JOIN recordings r ON r.id = ec.recording_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut have_chunks: std::collections::HashSet<String> = std::collections::HashSet::new();
        for row in chunk_rows {
            let id: String = row.try_get("id")?;
            let bytes: Vec<u8> = row.try_get("vector")?;
            let meeting_id: Option<String> = row.try_get("meeting_id")?;
            have_chunks.insert(id.clone());
            consider(id, meeting_id, bytes);
        }

        // Legacy whole-recording vectors, only for recordings not yet chunked, so
        // the library stays searchable while the backfill runs.
        let legacy_rows = sqlx::query(
            "SELECT e.id AS id, e.vector AS vector, r.meeting_id AS meeting_id \
             FROM embeddings e JOIN recordings r ON r.id = e.id",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in legacy_rows {
            let id: String = row.try_get("id")?;
            if have_chunks.contains(&id) {
                continue; // chunks supersede the legacy whole-recording vector
            }
            let bytes: Vec<u8> = row.try_get("vector")?;
            let meeting_id: Option<String> = row.try_get("meeting_id")?;
            consider(id, meeting_id, bytes);
        }

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
    async fn lexical_ranking(&self, query: &str) -> Result<Vec<(String, RecordingId)>> {
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

    /// Hybrid semantic + lexical search with Reciprocal Rank Fusion.
    ///
    /// This is the search the daemon now uses. It:
    /// 1. Ranks recordings by best-matching chunk cosine (paraphrase recall).
    /// 2. Ranks recordings by FTS5 BM25 (exact-term recall).
    /// 3. Fuses the two rankings with RRF (no fragile cross-scale threshold).
    /// 4. Returns `(RecordingId, relevance)` where `relevance` is the *calibrated*
    ///    best-chunk cosine (0..1) for display — a meaningful percentage, not raw
    ///    cosine. Lexical-only hits (no vector signal) get a small floor relevance
    ///    so they still surface with an honest "weak semantic match" reading.
    ///
    /// Performs a hybrid search over the recording catalog, combining semantic
    /// (vector) search and lexical (FTS5) search results.
    ///
    /// This function implements Reciprocal Rank Fusion (RRF) to merge the ordered
    /// listings from two distinct retrievers:
    /// 1. A vector-based semantic retriever that scores using cosine similarity
    ///    over ONNX embedding chunks (see [`crate::embed::Embedder`]).
    /// 2. A lexical prefix query over the FTS5 full-text search virtual table.
    ///
    /// ### Meeting Collapsing
    /// In Meeting Mode, a single meeting has two separate tracks (microphone and
    /// system loopback). Returning both tracks as separate search results would
    /// clutter the UI. To prevent this, results are grouped by a stable deduplication
    /// key (the `meeting_id` for meetings, or the `id` for standalone voice notes).
    /// If both retrievers match different tracks of the same meeting, the results
    /// collapse, and we return a single representative `RecordingId` (preferring
    /// the track with the strongest semantic match).
    ///
    /// ### Relevance Calibration & Flooring
    /// The `min_relevance` parameter filters out weak semantic hits (whose calibrated
    /// cosine score falls below the floor). Crucially, exact term matches from the
    /// lexical retriever are exempt from this threshold — if a user searches for an
    /// exact word that is present in the transcript, it is returned even if the
    /// semantic similarity score is low.
    pub async fn hybrid_search(
        &self,
        query: &str,
        query_vec: &[f32],
        limit: usize,
        min_relevance: f32,
    ) -> Result<Vec<(RecordingId, f32)>> {
        let vec_rank = self.vector_ranking(query_vec).await?;
        let lex_rank = self.lexical_ranking(query).await?;

        // Everything below is keyed by the meeting-stable DEDUPE KEY (meeting_id
        // or recording id), not the raw recording id, so a meeting collapses to a
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

    pub async fn list(&self, filter: &ListFilter) -> Result<Vec<Recording>> {
        let mut sql = String::from("SELECT recordings.* FROM recordings");

        let mut fts_query = None;
        let mut tag_search_query = None;

        if let Some(q) = filter.search.as_deref() {
            let sanitized = sanitize_fts5_query(q);
            if !sanitized.is_empty() {
                fts_query = Some(sanitized);
                tag_search_query = Some(format!("%{}%", q));
            }
        }

        if filter.tag_id.is_some() {
            sql.push_str(" JOIN recording_tags rt ON rt.recording_id = recordings.id");
        }

        sql.push_str(" WHERE 1=1");

        if fts_query.is_some() {
            sql.push_str(" AND (recordings.rowid IN (SELECT rowid FROM recordings_fts WHERE transcript MATCH ?) OR recordings.id IN (SELECT recording_id FROM recording_tags rts JOIN tags ts ON ts.id = rts.tag_id WHERE ts.name LIKE ?))");
        }
        if let Some(tag_id) = filter.tag_id {
            sql.push_str(&format!(" AND rt.tag_id = {tag_id}"));
        }
        if filter.status.is_some() {
            sql.push_str(" AND recordings.status = ?");
        }
        if filter.since.is_some() {
            sql.push_str(" AND recordings.started_at >= ?");
        }
        if filter.until.is_some() {
            sql.push_str(" AND recordings.started_at <= ?");
        }
        let dir = if filter.sort_desc.unwrap_or(true) {
            "DESC"
        } else {
            "ASC"
        };
        sql.push_str(&format!(
            " ORDER BY recordings.started_at {dir}, recordings.id {dir}"
        ));
        // LIMIT / OFFSET for pagination. SQLite requires a LIMIT before an
        // OFFSET, so when only an offset is given we use `LIMIT -1` (= no row
        // cap). `limit`/`offset` are `u32`, so direct formatting is injection-safe.
        match (filter.limit, filter.offset) {
            (Some(n), Some(m)) => sql.push_str(&format!(" LIMIT {n} OFFSET {m}")),
            (Some(n), None) => sql.push_str(&format!(" LIMIT {n}")),
            (None, Some(m)) => sql.push_str(&format!(" LIMIT -1 OFFSET {m}")),
            (None, None) => {}
        }

        let mut q = sqlx::query(&sql);
        if let Some(fq) = &fts_query {
            q = q.bind(fq);
        }
        if let Some(tq) = &tag_search_query {
            q = q.bind(tq);
        }
        if let Some(s) = filter.status {
            q = q.bind(s.as_str().to_string());
        }
        if let Some(t) = filter.since {
            q = q.bind(t.to_rfc3339());
        }
        if let Some(t) = filter.until {
            q = q.bind(t.to_rfc3339());
        }
        let rows = q.fetch_all(&self.pool).await?;
        let mut recs: Vec<Recording> = rows
            .into_iter()
            .map(row_to_recording)
            .collect::<Result<_>>()?;
        // Populate tags + custom speaker names for each recording (N+1 query;
        // acceptable for desktop UI scale).
        for rec in &mut recs {
            rec.tags = self.tags_for(&rec.id).await.unwrap_or_default();
            rec.speaker_names = self.speaker_names_for(&rec.id).await.unwrap_or_default();
        }
        Ok(recs)
    }

    /// Fetch all recordings belonging to a single meeting session.
    ///
    /// Returns the rows that share `meeting_id`, ordered by `track` then
    /// `started_at` so the two tracks of a meeting come back in a stable order
    /// (e.g. "mic" before "system", since "mic" < "system" lexicographically).
    /// A `meeting_id` with no rows yields an empty `Vec` (not an error) — the
    /// caller treats that as "no such session".
    pub async fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<Recording>> {
        let rows = sqlx::query(
            "SELECT * FROM recordings WHERE meeting_id = ? \
             ORDER BY track ASC, started_at ASC, id ASC",
        )
        .bind(meeting_id)
        .fetch_all(&self.pool)
        .await?;
        let mut recs: Vec<Recording> = rows
            .into_iter()
            .map(row_to_recording)
            .collect::<Result<_>>()?;
        // The merged meeting view maps `[Speaker N]` → custom names per track, so
        // each track must carry its own speaker-name map.
        for rec in &mut recs {
            rec.speaker_names = self.speaker_names_for(&rec.id).await.unwrap_or_default();
        }
        Ok(recs)
    }

    pub async fn delete(&self, id: &RecordingId) -> Result<()> {
        sqlx::query("DELETE FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Run an explicit WAL checkpoint. PASSIVE mode is non-blocking — readers
    /// can keep going while the checkpoint runs. The daemon calls this on idle
    /// (e.g., when the queue worker has been quiet for a few minutes) to keep
    /// the `-wal` file from growing unbounded under sustained read pressure
    /// from `SubscribeEvents` subscribers.
    pub async fn checkpoint(&self) -> Result<()> {
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_tags(&self) -> Result<Vec<Tag>> {
        // Only return tags that are attached to at least one recording.
        // Orphaned tags (detached from all recordings) are excluded so they
        // don't pollute the filter dropdown or tag autocomplete.
        let rows = sqlx::query(
            "SELECT id, name, color FROM tags \
             WHERE id IN (SELECT tag_id FROM recording_tags) \
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Tag {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    color: r.try_get("color")?,
                })
            })
            .collect()
    }

    pub async fn add_tag(&self, name: &str, color: Option<&str>) -> Result<Tag> {
        // Tags are case-INSENSITIVELY unique at the application level: "Code"
        // and "code" are the same tag, so adding either reuses the existing
        // row (keeping its color and recording links — the first-created
        // casing wins). The UNIQUE index on `name` is byte-wise, which is why
        // this lookup guards the insert. COLLATE NOCASE is ASCII-only; that
        // covers the realistic duplicate ("Test"/"test") without rewriting
        // non-ASCII tag names.
        let existing =
            sqlx::query("SELECT id, name, color FROM tags WHERE name = ? COLLATE NOCASE")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        if let Some(row) = existing {
            return Ok(Tag {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                color: row.try_get("color")?,
            });
        }
        sqlx::query("INSERT OR IGNORE INTO tags (name, color) VALUES (?, ?)")
            .bind(name)
            .bind(color)
            .execute(&self.pool)
            .await?;
        let row = sqlx::query("SELECT id, name, color FROM tags WHERE name = ?")
            .bind(name)
            .fetch_one(&self.pool)
            .await?;
        Ok(Tag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            color: row.try_get("color")?,
        })
    }

    pub async fn update_tag(&self, id: i64, name: &str, color: Option<&str>) -> Result<Tag> {
        sqlx::query("UPDATE tags SET name = ?, color = ? WHERE id = ?")
            .bind(name)
            .bind(color)
            .bind(id)
            .execute(&self.pool)
            .await?;
        let row = sqlx::query("SELECT id, name, color FROM tags WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(Tag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            color: row.try_get("color")?,
        })
    }

    pub async fn delete_tag(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Returns ALL tags including ones not attached to any recording.
    /// Used by the Tag Manager settings UI.
    pub async fn list_all_tags(&self) -> Result<Vec<Tag>> {
        let rows = sqlx::query("SELECT id, name, color FROM tags ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Tag {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    color: r.try_get("color")?,
                })
            })
            .collect()
    }

    pub async fn attach_tag(&self, recording_id: &RecordingId, tag_id: i64) -> Result<()> {
        sqlx::query("INSERT OR IGNORE INTO recording_tags (recording_id, tag_id) VALUES (?, ?)")
            .bind(recording_id.as_str())
            .bind(tag_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn detach_tag(&self, recording_id: &RecordingId, tag_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM recording_tags WHERE recording_id = ? AND tag_id = ?")
            .bind(recording_id.as_str())
            .bind(tag_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Number of recordings each tag is attached to, keyed by tag id. Tags with
    /// no attachments are simply absent from the map (treated as zero by callers).
    /// Powers the Tag Manager usage counts.
    pub async fn tag_usage_counts(&self) -> Result<std::collections::HashMap<i64, i64>> {
        let rows =
            sqlx::query("SELECT tag_id, COUNT(*) AS cnt FROM recording_tags GROUP BY tag_id")
                .fetch_all(&self.pool)
                .await?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for r in rows {
            let id: i64 = r.try_get("tag_id")?;
            let cnt: i64 = r.try_get("cnt")?;
            map.insert(id, cnt);
        }
        Ok(map)
    }

    /// Merge `from_id` into `into_id`: every recording tagged `from_id` becomes
    /// tagged `into_id` (de-duplicated), then `from_id` is deleted. A no-op when
    /// the two ids are equal. Used by the Tag Manager's merge action.
    pub async fn merge_tags(&self, from_id: i64, into_id: i64) -> Result<()> {
        if from_id == into_id {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        // Re-point every link from the source tag onto the target, skipping rows
        // that would collide with an existing (recording_id, into_id) pair.
        sqlx::query(
            "INSERT OR IGNORE INTO recording_tags (recording_id, tag_id) \
             SELECT recording_id, ? FROM recording_tags WHERE tag_id = ?",
        )
        .bind(into_id)
        .bind(from_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM recording_tags WHERE tag_id = ?")
            .bind(from_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(from_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Apply the configured retention policy, removing eligible recordings from
    /// the catalog and returning their `audio_path` values so the caller can
    /// delete the files from disk.
    ///
    /// Only terminal-state recordings (done / failed) are eligible — in-progress
    /// recordings are always preserved regardless of age or count.
    pub async fn apply_retention(
        &self,
        cfg: &crate::config::RetentionConfig,
    ) -> Result<Vec<String>> {
        let mut deleted_paths: Vec<String> = Vec::new();

        // `delete_audio = true` is the disk-saver mode: the catalog row stays
        // (transcript searchable forever), only the WAV goes. The row's
        // audio_path is blanked so the UI doesn't offer a dead player, and so
        // the row never matches a later sweep again. `false` (default) deletes
        // row + audio together. This flag was previously ignored — audio-only
        // users were losing their rows.
        let audio_only = cfg.delete_audio;

        // Age-based cleanup — everything older than max_age_days.
        if let Some(max_age) = cfg.max_age_days {
            let cutoff =
                chrono::Utc::now() - chrono::Duration::try_days(max_age as i64).unwrap_or_default();
            let cutoff_str = cutoff.to_rfc3339();
            let rows = sqlx::query(
                "SELECT id, audio_path FROM recordings \
                 WHERE started_at < ? AND status IN ('done','transcribe_failed','hook_failed') \
                 AND audio_path != ''",
            )
            .bind(&cutoff_str)
            .fetch_all(&self.pool)
            .await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let audio_path: String = row.try_get("audio_path")?;
                if audio_only {
                    sqlx::query(
                        "UPDATE recordings SET audio_path = '', updated_at = datetime('now') \
                         WHERE id = ?",
                    )
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                } else {
                    sqlx::query("DELETE FROM recordings WHERE id = ?")
                        .bind(&id)
                        .execute(&self.pool)
                        .await?;
                }
                deleted_paths.push(audio_path);
            }
        }

        // Count-based cleanup — all but the most recent max_count. In
        // audio-only mode the ranking still counts EVERY terminal row (rows
        // are kept, so "the most recent N" must mean recordings, not files) —
        // the audio_path filter above/below only stops re-processing rows
        // whose audio is already gone.
        if let Some(max_count) = cfg.max_count {
            let rows = sqlx::query(
                "SELECT id, audio_path FROM recordings \
                 WHERE status IN ('done','transcribe_failed','hook_failed') \
                 ORDER BY started_at DESC, id DESC \
                 LIMIT -1 OFFSET ?",
            )
            .bind(max_count as i64)
            .fetch_all(&self.pool)
            .await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let audio_path: String = row.try_get("audio_path")?;
                if audio_path.is_empty() {
                    continue; // audio already reclaimed by an earlier sweep
                }
                if audio_only {
                    sqlx::query(
                        "UPDATE recordings SET audio_path = '', updated_at = datetime('now') \
                         WHERE id = ?",
                    )
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                } else {
                    sqlx::query("DELETE FROM recordings WHERE id = ?")
                        .bind(&id)
                        .execute(&self.pool)
                        .await?;
                }
                deleted_paths.push(audio_path);
            }
        }

        Ok(deleted_paths)
    }

    /// Predicts how many recordings will be deleted by the age-based retention policy
    /// in the next `hours_ahead` hours.
    pub async fn analyze_upcoming_retention(
        &self,
        cfg: &crate::config::RetentionConfig,
        hours_ahead: u32,
    ) -> Result<u32> {
        let max_age = match cfg.max_age_days {
            Some(v) => v,
            None => return Ok(0),
        };

        // cutoff_now is items older than this are ALREADY deleted or being deleted now.
        let cutoff_now =
            chrono::Utc::now() - chrono::Duration::try_days(max_age as i64).unwrap_or_default();
        // cutoff_future is items older than this will be deleted in the next `hours_ahead` hours.
        let cutoff_future =
            cutoff_now + chrono::Duration::try_hours(hours_ahead as i64).unwrap_or_default();

        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM recordings \
             WHERE started_at >= ? AND started_at < ? \
             AND status IN ('done','transcribe_failed','hook_failed')",
        )
        .bind(cutoff_now.to_rfc3339())
        .bind(cutoff_future.to_rfc3339())
        .fetch_one(&self.pool)
        .await?;

        Ok(count as u32)
    }

    pub async fn tags_for(&self, recording_id: &RecordingId) -> Result<Vec<Tag>> {
        let rows = sqlx::query(
            r#"SELECT t.id, t.name, t.color
               FROM tags t
               JOIN recording_tags rt ON rt.tag_id = t.id
               WHERE rt.recording_id = ?
               ORDER BY t.name"#,
        )
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Tag {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    color: r.try_get("color")?,
                })
            })
            .collect()
    }

    /// Set (or clear) the custom display name for one diarized speaker label.
    ///
    /// `speaker_label` is the 1-based index from the transcript's `[Speaker N]`
    /// marker. A non-empty `name` upserts the mapping; a blank/whitespace-only
    /// `name` deletes it (the label reverts to the default "Speaker N"). The
    /// stored transcript is never touched — names are applied at display/export
    /// time — so renaming is fully reversible. The recording is expected to
    /// exist; a foreign-key violation surfaces as an error.
    pub async fn set_speaker_name(
        &self,
        recording_id: &RecordingId,
        speaker_label: i64,
        name: &str,
    ) -> Result<()> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            sqlx::query("DELETE FROM speaker_names WHERE recording_id = ? AND speaker_label = ?")
                .bind(recording_id.as_str())
                .bind(speaker_label)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO speaker_names (recording_id, speaker_label, name) VALUES (?, ?, ?) \
                 ON CONFLICT(recording_id, speaker_label) DO UPDATE SET name = excluded.name",
            )
            .bind(recording_id.as_str())
            .bind(speaker_label)
            .bind(trimmed)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// All custom speaker names for a recording, ordered by speaker index. Empty
    /// when none have been set. Used to populate `Recording::speaker_names` and
    /// by the IPC layer so the frontend can map `[Speaker N]` → name at display
    /// and export time.
    pub async fn speaker_names_for(&self, recording_id: &RecordingId) -> Result<Vec<SpeakerName>> {
        let rows = sqlx::query(
            "SELECT speaker_label, name FROM speaker_names \
             WHERE recording_id = ? ORDER BY speaker_label",
        )
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(SpeakerName {
                    speaker_label: r.try_get("speaker_label")?,
                    name: r.try_get("name")?,
                })
            })
            .collect()
    }

    /// Replace a recording's machine transcript segments with a fresh set.
    ///
    /// Called by the pipeline after every transcribe/retranscribe — segments
    /// always describe the *current* machine output, so the old rows are
    /// dropped first (in the same transaction, so a crash can't leave a
    /// half-replaced timeline). An empty slice simply clears them (e.g. a
    /// provider that returns no timing data).
    pub async fn replace_segments(
        &self,
        recording_id: &RecordingId,
        segments: &[TranscriptSegment],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM transcript_segments WHERE recording_id = ?")
            .bind(recording_id.as_str())
            .execute(&mut *tx)
            .await?;
        for (idx, seg) in segments.iter().enumerate() {
            sqlx::query(
                "INSERT INTO transcript_segments (recording_id, idx, start_ms, end_ms, text, speaker) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(recording_id.as_str())
            .bind(idx as i64)
            .bind(seg.start_ms)
            .bind(seg.end_ms)
            .bind(&seg.text)
            .bind(&seg.speaker)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A recording's machine transcript segments in timeline order. Empty when
    /// the recording predates segment capture or its provider returned no
    /// timing data — callers must treat "no segments" as a normal state, not
    /// an error.
    pub async fn segments_for(&self, recording_id: &RecordingId) -> Result<Vec<TranscriptSegment>> {
        let rows = sqlx::query(
            "SELECT start_ms, end_ms, text, speaker FROM transcript_segments \
             WHERE recording_id = ? ORDER BY idx",
        )
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(TranscriptSegment {
                    start_ms: r.try_get("start_ms")?,
                    end_ms: r.try_get("end_ms")?,
                    text: r.try_get("text")?,
                    speaker: r.try_get("speaker")?,
                })
            })
            .collect()
    }
}

fn row_to_recording(row: sqlx::sqlite::SqliteRow) -> Result<Recording> {
    let id: String = row.try_get("id")?;
    let started_at: String = row.try_get("started_at")?;
    let status: String = row.try_get("status")?;
    Ok(Recording {
        id: RecordingId::from_str_unchecked(&id),
        started_at: parse_dt(&started_at)?,
        duration_ms: row.try_get("duration_ms")?,
        audio_path: row.try_get("audio_path")?,
        transcript: row.try_get("transcript")?,
        model: row.try_get("model")?,
        status: parse_status(&status)?,
        error_kind: row.try_get("error_kind")?,
        error_message: row.try_get("error_message")?,
        hook_command: row.try_get("hook_command")?,
        hook_exit_code: row.try_get("hook_exit_code")?,
        hook_duration_ms: row.try_get("hook_duration_ms")?,
        transcribed_at: row
            .try_get::<Option<String>, _>("transcribed_at")?
            .map(|s| parse_dt(&s))
            .transpose()?,
        hook_ran_at: row
            .try_get::<Option<String>, _>("hook_ran_at")?
            .map(|s| parse_dt(&s))
            .transpose()?,
        notes: row.try_get("notes")?,
        meeting_id: row.try_get("meeting_id")?,
        meeting_name: row.try_get("meeting_name")?,
        track: row.try_get("track")?,
        in_place: row.try_get("in_place").unwrap_or(false),
        cleanup_model: row.try_get("cleanup_model").unwrap_or(None),
        diarized: row.try_get("diarized").unwrap_or(false),
        user_edited: row.try_get("user_edited").unwrap_or(false),
        favorite: row.try_get("favorite").unwrap_or(false),
        tag_suggestions: row
            .try_get::<Option<String>, _>("tag_suggestions")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        summary: row.try_get("summary").unwrap_or(None),
        summary_model: row.try_get("summary_model").unwrap_or(None),
        title: row.try_get("title").unwrap_or(None),
        title_is_auto: row.try_get("title_is_auto").unwrap_or(true),
        tags: Vec::new(),
        // Populated separately (joined from `speaker_names`) by list/get/list_by_meeting.
        speaker_names: Vec::new(),
    })
}

fn parse_dt(s: &str) -> Result<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Local))
        .or_else(|_| {
            // SQLite's datetime('now') returns "YYYY-MM-DD HH:MM:SS" UTC.
            let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map_err(|e| crate::error::Error::Internal(format!("bad datetime {s}: {e}")))?;
            Ok(chrono::TimeZone::from_utc_datetime(&chrono::Utc, &naive).with_timezone(&Local))
        })
}

fn parse_status(s: &str) -> Result<RecordingStatus> {
    Ok(match s {
        "recording" => RecordingStatus::Recording,
        "paused" => RecordingStatus::Paused,
        "transcribing" => RecordingStatus::Transcribing,
        "cleaning_up" => RecordingStatus::CleaningUp,
        "summarizing" => RecordingStatus::Summarizing,
        "tagging" => RecordingStatus::Tagging,
        "hook_running" => RecordingStatus::HookRunning,
        "done" => RecordingStatus::Done,
        "transcribe_failed" => RecordingStatus::TranscribeFailed,
        "hook_failed" => RecordingStatus::HookFailed,
        other => {
            return Err(crate::error::Error::Internal(format!(
                "unknown recording status: {other}"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_round_trips_all_variants_incl_paused() {
        // Regression: `parse_status` lacked a "paused" arm, so the moment a
        // recording was paused, `catalog.list()`/`get()` failed for the ENTIRE
        // result set (one bad row errored the whole query). Every status the DB
        // can hold must round-trip.
        for s in [
            "recording",
            "paused",
            "transcribing",
            "cleaning_up",
            "summarizing",
            "tagging",
            "hook_running",
            "done",
            "transcribe_failed",
            "hook_failed",
        ] {
            assert_eq!(
                parse_status(s).unwrap().as_str(),
                s,
                "status {s} did not round-trip through parse_status/as_str"
            );
        }
    }

    #[test]
    fn test_sanitize_fts5_query() {
        assert_eq!(sanitize_fts5_query("hello"), "hello*");
        assert_eq!(sanitize_fts5_query("hello world"), "hello* AND world*");
        assert_eq!(sanitize_fts5_query("O'Connor"), "O* AND Connor*");
        assert_eq!(
            sanitize_fts5_query("some-bad*characters"),
            "some* AND bad* AND characters*"
        );
        assert_eq!(sanitize_fts5_query("\"quotes\""), "quotes*");
        assert_eq!(sanitize_fts5_query("   spaces   "), "spaces*");
        assert_eq!(sanitize_fts5_query(""), "");
    }

    /// A minimal `Done` recording for embedding/search tests. `semantic_search`
    /// JOINs embeddings to recordings, so the row must exist before embedding.
    fn embedded_recording(meeting_id: Option<&str>) -> Recording {
        Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: "x.wav".into(),
            transcript: Some("t".into()),
            model: Some("tiny".into()),
            status: RecordingStatus::Done,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: meeting_id.map(|s| s.to_string()),
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            tags: vec![],
            speaker_names: vec![],
        }
    }

    #[tokio::test]
    async fn semantic_search_ranks_by_cosine_and_respects_limit() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        let b = embedded_recording(None);
        let c = embedded_recording(None);
        for r in [&a, &b, &c] {
            db.insert(r).await.unwrap();
        }
        // Orthonormal vectors: query [1,0,0] is identical to `a`, orthogonal to b/c.
        db.upsert_embedding(&a.id, &[1.0, 0.0, 0.0]).await.unwrap();
        db.upsert_embedding(&b.id, &[0.0, 1.0, 0.0]).await.unwrap();
        db.upsert_embedding(&c.id, &[0.0, 0.0, 1.0]).await.unwrap();

        // min_score -1.0 keeps everything so we can assert ordering.
        let results = db
            .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0.as_str(), a.id.as_str(), "best match first");
        assert!(
            (results[0].1 - 1.0).abs() < 1e-6,
            "identical vector scores ~1.0"
        );
        assert!(
            results[0].1 >= results[1].1 && results[1].1 >= results[2].1,
            "results must be sorted high→low"
        );

        // `limit` caps the result count.
        let top1 = db.semantic_search(&[1.0, 0.0, 0.0], 1, -1.0).await.unwrap();
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].0.as_str(), a.id.as_str());
    }

    #[tokio::test]
    async fn semantic_search_min_score_filters_low_matches() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.upsert_embedding(&a.id, &[1.0, 0.0, 0.0]).await.unwrap();
        // Orthogonal query → cosine 0.0, under a 0.5 floor → dropped.
        let results = db.semantic_search(&[0.0, 1.0, 0.0], 10, 0.5).await.unwrap();
        assert!(
            results.is_empty(),
            "below-floor matches must be filtered out"
        );
    }

    #[tokio::test]
    async fn semantic_search_skips_dimension_mismatch_without_panicking() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let good = embedded_recording(None);
        let bad = embedded_recording(None);
        db.insert(&good).await.unwrap();
        db.insert(&bad).await.unwrap();
        db.upsert_embedding(&good.id, &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        // Wrong dimension (2 vs the query's 3) — must be skipped, not scored on a
        // truncated prefix and not panic.
        db.upsert_embedding(&bad.id, &[1.0, 0.0]).await.unwrap();

        let results = db
            .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "the mismatched-dim embedding is skipped");
        assert_eq!(results[0].0.as_str(), good.id.as_str());
    }

    #[tokio::test]
    async fn semantic_search_collapses_meeting_tracks() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mic = embedded_recording(Some("meeting-1"));
        let sys = embedded_recording(Some("meeting-1"));
        let solo = embedded_recording(None);
        for r in [&mic, &sys, &solo] {
            db.insert(r).await.unwrap();
        }
        // Both tracks of meeting-1 are highly similar to the query; solo isn't.
        db.upsert_embedding(&mic.id, &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        db.upsert_embedding(&sys.id, &[0.99, 0.01, 0.0])
            .await
            .unwrap();
        db.upsert_embedding(&solo.id, &[0.0, 1.0, 0.0])
            .await
            .unwrap();

        let results = db
            .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
            .await
            .unwrap();
        // The meeting's two tracks collapse to one entry (best-scoring track),
        // alongside the standalone recording.
        assert_eq!(results.len(), 2);
        let meeting_hits = results
            .iter()
            .filter(|(id, _)| id.as_str() == mic.id.as_str() || id.as_str() == sys.id.as_str())
            .count();
        assert_eq!(
            meeting_hits, 1,
            "meeting tracks must collapse to one result"
        );
    }

    #[tokio::test]
    async fn upsert_chunk_embeddings_replaces_prior_chunks() {
        // Re-embedding (a re-transcription or a manual edit) must REPLACE a
        // recording's chunk vectors, never leave stale ones from the old text
        // behind — otherwise an edited note keeps matching phrases it no longer
        // contains. We store three chunks, then re-embed with two and assert the
        // third is gone.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();

        db.upsert_chunk_embeddings(
            &a.id,
            &[
                vec![1.0, 0.0, 0.0],
                vec![0.0, 1.0, 0.0],
                vec![0.0, 0.0, 1.0],
            ],
        )
        .await
        .unwrap();

        // A query identical to the second chunk finds the recording.
        let r = db.vector_ranking(&[0.0, 1.0, 0.0]).await.unwrap();
        assert_eq!(r.len(), 1);
        assert!((r[0].2 - 1.0).abs() < 1e-6, "best chunk is the exact match");

        // Re-embed with only two chunks; the third (z-axis) must be dropped.
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();
        // The old z-axis chunk is gone: a z-axis query now only matches by the
        // shared positive baseline (here, exactly 0 against the two remaining
        // orthogonal chunks), not 1.0.
        let r2 = db.vector_ranking(&[0.0, 0.0, 1.0]).await.unwrap();
        assert!(
            r2.is_empty() || r2[0].2 < 0.5,
            "stale chunk must not survive a re-embed (got {r2:?})"
        );

        // Empty re-embed clears all chunks.
        db.upsert_chunk_embeddings(&a.id, &[]).await.unwrap();
        let none = db.list_recordings_without_chunk_embeddings().await.unwrap();
        assert!(
            none.iter().any(|rec| rec.id.as_str() == a.id.as_str()),
            "after clearing, the recording reappears as needing chunks"
        );
    }

    #[tokio::test]
    async fn vector_ranking_scores_by_best_chunk_not_average() {
        // The core paraphrase fix: a recording is ranked by its BEST-matching
        // chunk (max-sim), not by an averaged whole-note vector. Recording `a`
        // has many unrelated chunks plus ONE chunk that nails the query; it must
        // still rank top, because that one chunk competes on its own tight vector
        // instead of being diluted by the rest of the note.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        let b = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.insert(&b).await.unwrap();

        // `a`: one chunk exactly on the query axis, several pulling other ways.
        db.upsert_chunk_embeddings(
            &a.id,
            &[
                vec![0.0, 1.0, 0.0],
                vec![0.0, 0.0, 1.0],
                vec![1.0, 0.0, 0.0], // the matching chunk
                vec![0.0, 1.0, 0.0],
            ],
        )
        .await
        .unwrap();
        // `b`: a single chunk only loosely aligned with the query.
        db.upsert_chunk_embeddings(&b.id, &[vec![0.6, 0.8, 0.0]])
            .await
            .unwrap();

        let ranking = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(ranking.len(), 2);
        assert_eq!(
            ranking[0].1.as_str(),
            a.id.as_str(),
            "the recording with the best single chunk wins (max-sim, not mean)"
        );
        assert!(
            (ranking[0].2 - 1.0).abs() < 1e-6,
            "best-chunk cosine is the exact-match chunk's score, not an average"
        );
    }

    #[tokio::test]
    async fn vector_ranking_falls_back_to_legacy_whole_recording_vector() {
        // During the backfill window a recording may still have only a legacy
        // whole-recording vector and no chunks. It must remain searchable via the
        // `embeddings` table fallback, and once chunks exist they SUPERSEDE the
        // legacy vector (no double-counting / no stale legacy score winning).
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let legacy_only = embedded_recording(None);
        let chunked = embedded_recording(None);
        db.insert(&legacy_only).await.unwrap();
        db.insert(&chunked).await.unwrap();

        // legacy_only: only the old whole-recording vector, loosely on-axis.
        db.upsert_embedding(&legacy_only.id, &[0.8, 0.6, 0.0])
            .await
            .unwrap();
        // chunked: a stale legacy vector AND a fresh, better chunk vector. The
        // chunk must win; the legacy row must be ignored for this recording.
        db.upsert_embedding(&chunked.id, &[0.0, 0.0, 1.0])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&chunked.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();

        let ranking = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(ranking.len(), 2, "both recordings are searchable");
        // chunked's fresh chunk (cosine 1.0) beats legacy_only's 0.8.
        assert_eq!(ranking[0].1.as_str(), chunked.id.as_str());
        assert!((ranking[0].2 - 1.0).abs() < 1e-6);
        // And the chunked recording is scored from its chunk, not its stale
        // legacy vector (which was orthogonal → would have scored 0.0).
        let legacy_score = ranking
            .iter()
            .find(|(_key, id, _score)| id.as_str() == legacy_only.id.as_str())
            .unwrap()
            .2;
        assert!(
            (legacy_score - 0.8).abs() < 1e-6,
            "legacy-only recording scored from its whole-recording vector"
        );
    }

    #[tokio::test]
    async fn hybrid_search_recalls_a_paraphrase_where_keyword_match_misses() {
        // THE headline requirement: "utter the likeness of something I spoke
        // about and get the proper search results."
        //
        // We simulate the embedding space directly (the ONNX model isn't bundled
        // in tests). The query and the target recording's transcript share NO
        // word, so FTS5 (lexical) returns nothing for them — a naive keyword
        // search misses entirely. But their *vectors* are nearly identical
        // (high cosine), modelling a paraphrase. Hybrid search must still surface
        // the right recording, ranked first, with an honest relevance score.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

        // The recording the user is trying to recall. Its transcript talks about
        // moving the schema over — the QUERY below ("database migration") shares
        // none of these words, so lexical search cannot find it.
        let mut target = embedded_recording(None);
        target.transcript = Some("we should shift the records across to the new store".into());
        // A distractor whose words overlap the query's domain words a bit but
        // whose meaning (and vector) is unrelated.
        let mut distractor = embedded_recording(None);
        distractor.transcript = Some("lunch plans for friday afternoon".into());
        db.insert(&target).await.unwrap();
        db.insert(&distractor).await.unwrap();

        // Query vector ("the bit about the database migration"). The target's
        // matching chunk vector is nearly identical (paraphrase); the distractor
        // points elsewhere.
        let query_vec = [1.0_f32, 0.0, 0.0];
        db.upsert_chunk_embeddings(&target.id, &[vec![0.98, 0.20, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&distractor.id, &[vec![0.0, 0.0, 1.0]])
            .await
            .unwrap();

        // Sanity: a pure keyword search for the query terms finds NOTHING — the
        // words don't appear in either transcript. This is the gap vectors close.
        let lexical = db.lexical_ranking("database migration").await.unwrap();
        assert!(
            lexical.is_empty(),
            "precondition: naive keyword search must miss the paraphrase"
        );

        // Hybrid search, same min_relevance the daemon uses (0.12). Despite the
        // lexical miss, the semantic signal surfaces the target, ranked first.
        let results = db
            .hybrid_search("database migration", &query_vec, 10, 0.12)
            .await
            .unwrap();
        assert!(
            !results.is_empty(),
            "paraphrase must be recalled by meaning"
        );
        assert_eq!(
            results[0].0.as_str(),
            target.id.as_str(),
            "the paraphrased recording must rank first"
        );
        // The displayed relevance is the calibrated best-chunk cosine — a strong
        // paraphrase (cosine ~0.98) should read as a strong match, not single
        // digits.
        assert!(
            results[0].1 > 0.5,
            "a strong paraphrase should read as a strong relevance, got {}",
            results[0].1
        );
    }

    #[tokio::test]
    async fn hybrid_search_keeps_exact_term_hit_despite_weak_cosine() {
        // The complement to paraphrase recall: when the user remembers one
        // distinctive word, an exact lexical hit must surface even if its vector
        // barely aligns with the query — never filtered out by the relevance
        // floor. This is the "union of strengths" guarantee.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mut named = embedded_recording(None);
        named.transcript = Some("the Kubernetes rollout notes are attached".into());
        db.insert(&named).await.unwrap();
        // Its vector is essentially orthogonal to the query (weak cosine), so a
        // semantic-only path with a 0.12 floor would drop it.
        db.upsert_chunk_embeddings(&named.id, &[vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();

        // The user types the exact distinctive term; the query vector is the
        // unrelated x-axis.
        let results = db
            .hybrid_search("Kubernetes", &[1.0, 0.0, 0.0], 10, 0.12)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "the exact-term hit must survive");
        assert_eq!(results[0].0.as_str(), named.id.as_str());
        assert!(
            results[0].1 > 0.0,
            "a lexical-only hit gets an honest non-zero relevance floor, not 0%"
        );
    }

    #[tokio::test]
    async fn hybrid_search_collapses_a_meeting_across_both_retrievers() {
        // Regression for the cross-retriever dedupe: a meeting's two tracks share
        // a meeting_id. If the vector retriever's best track differs from the
        // lexical retriever's best track, fusing on raw recording id would surface
        // the SAME meeting twice. Fusing on the meeting-stable dedupe key must
        // collapse it to one row.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mic = embedded_recording(Some("meeting-x"));
        let mut sys = embedded_recording(Some("meeting-x"));
        // Put the distinctive lexical term on the SYSTEM track only, and the
        // strong semantic vector on the MIC track only — so each retriever prefers
        // a different track of the same meeting.
        sys.transcript = Some("the quarterly Kubernetes review".into());
        db.insert(&mic).await.unwrap();
        db.insert(&sys).await.unwrap();

        // Mic track: chunk vector strongly on the query axis (semantic winner).
        db.upsert_chunk_embeddings(&mic.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        // System track: vector points elsewhere, but it carries the exact term.
        db.upsert_chunk_embeddings(&sys.id, &[vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();

        let results = db
            .hybrid_search("Kubernetes", &[1.0, 0.0, 0.0], 10, 0.12)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "the meeting's two tracks must collapse to a single result, got {results:?}"
        );
        // The surviving row is one of the meeting's tracks.
        assert!(
            results[0].0.as_str() == mic.id.as_str() || results[0].0.as_str() == sys.id.as_str()
        );
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let db = Catalog::open(Path::new("sqlite::memory:"))
            .await
            .expect("open db");
        let r = Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 5000,
            audio_path: "foo.wav".into(),
            transcript: Some("hello world".into()),
            model: Some("tiny".into()),
            status: RecordingStatus::Done,
            error_kind: None,
            error_message: None,
            hook_command: Some("to-stdout.ps1".into()),
            hook_exit_code: Some(0),
            hook_duration_ms: Some(100),
            transcribed_at: Some(Local::now()),
            hook_ran_at: Some(Local::now()),
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            tags: vec![],
            speaker_names: vec![],
        };
        db.insert(&r).await.expect("insert");

        let fetched = db
            .get(&r.id)
            .await
            .expect("get recording")
            .expect("is some");
        assert_eq!(fetched.id.as_str(), r.id.as_str());
        assert_eq!(fetched.audio_path, r.audio_path);
        assert_eq!(fetched.transcript.as_deref(), Some("hello world"));
        assert_eq!(fetched.status, RecordingStatus::Done);

        // Test list
        let filter = ListFilter {
            limit: Some(10),
            offset: None,
            since: None,
            until: None,
            status: None,
            search: None,
            tag_id: None,
            sort_desc: None,
        };
        let list = db.list(&filter).await.expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id.as_str(), r.id.as_str());
    }

    #[tokio::test]
    async fn original_transcript_preserved_across_user_edit() {
        let db = Catalog::open(Path::new("sqlite::memory:"))
            .await
            .expect("open db");
        let r = Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: "x.wav".into(),
            transcript: None,
            model: None,
            status: RecordingStatus::Transcribing,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            tags: vec![],
            speaker_names: vec![],
        };
        db.insert(&r).await.expect("insert");

        // Machine transcription stores transcript + original.
        db.update_transcript(&r.id, "machine output", "machine output", "ggml-base")
            .await
            .expect("machine transcript");
        assert_eq!(
            db.get_original_transcript(&r.id).await.unwrap().as_deref(),
            Some("machine output")
        );

        // A user edit changes the transcript but preserves the original.
        db.update_user_transcript(&r.id, "edited by the user")
            .await
            .expect("user edit");
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.transcript.as_deref(), Some("edited by the user"));
        // The transcription model is preserved — a hand edit is surfaced by the
        // user_edited flag / "Edited" column, not by overwriting the model field.
        assert_eq!(got.model.as_deref(), Some("ggml-base"));
        assert!(
            got.user_edited,
            "a manual edit must set the user_edited flag"
        );
        assert_eq!(
            db.get_original_transcript(&r.id).await.unwrap().as_deref(),
            Some("machine output")
        );
    }

    #[tokio::test]
    async fn notes_round_trip_and_survive_transcription() {
        let db = Catalog::open(Path::new("sqlite::memory:"))
            .await
            .expect("open db");
        let r = Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: "x.wav".into(),
            transcript: None,
            model: None,
            status: RecordingStatus::Transcribing,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            tags: vec![],
            speaker_names: vec![],
        };
        db.insert(&r).await.expect("insert");

        // Fresh insert: notes default to NULL.
        assert_eq!(db.get(&r.id).await.unwrap().unwrap().notes, None);

        // Notes round-trip through update_notes + get.
        db.update_notes(&r.id, "remember to follow up")
            .await
            .expect("update notes");
        assert_eq!(
            db.get(&r.id).await.unwrap().unwrap().notes.as_deref(),
            Some("remember to follow up")
        );

        // (Re-)transcription writes the transcript columns but must NOT touch notes.
        db.update_transcript(&r.id, "machine output", "machine output", "ggml-base")
            .await
            .expect("machine transcript");
        let after_transcribe = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(
            after_transcribe.transcript.as_deref(),
            Some("machine output")
        );
        assert_eq!(
            after_transcribe.notes.as_deref(),
            Some("remember to follow up"),
            "re-transcription must not clear notes"
        );

        // A manual transcript edit must also preserve notes.
        db.update_user_transcript(&r.id, "edited by the user")
            .await
            .expect("user edit");
        assert_eq!(
            db.get(&r.id).await.unwrap().unwrap().notes.as_deref(),
            Some("remember to follow up"),
            "user transcript edit must not clear notes"
        );
    }

    #[tokio::test]
    async fn set_title_auto_writes_never_overwrite_a_user_title() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // Fresh rows are untitled and auto-owned (the migration default).
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.title, None);
        assert!(got.title_is_auto, "fresh rows must be auto-owned");

        // An auto write lands while the title is auto-owned (and a later auto
        // write — e.g. a retranscribe — refreshes it).
        assert!(db.set_title(&r.id, Some("first pass"), true).await.unwrap());
        assert!(db
            .set_title(&r.id, Some("second pass"), true)
            .await
            .unwrap());
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.title.as_deref(), Some("second pass"));
        assert!(got.title_is_auto);

        // The user takes ownership; from now on auto writes are no-ops.
        assert!(db.set_title(&r.id, Some("My title"), false).await.unwrap());
        assert!(
            !db.set_title(&r.id, Some("auto again"), true).await.unwrap(),
            "an auto write must be skipped once the user owns the title"
        );
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.title.as_deref(), Some("My title"));
        assert!(!got.title_is_auto, "title_is_auto = 0 wins forever");

        // Clearing (None) empties the title and reverts ownership to auto, so
        // the next pipeline run may fill it again.
        assert!(db.set_title(&r.id, None, true).await.unwrap());
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.title, None);
        assert!(got.title_is_auto, "a cleared title reverts to auto-owned");
        assert!(db.set_title(&r.id, Some("fresh auto"), true).await.unwrap());
        assert_eq!(
            db.get(&r.id).await.unwrap().unwrap().title.as_deref(),
            Some("fresh auto")
        );

        // Unknown ids report no update.
        assert!(!db
            .set_title(&RecordingId::new(), Some("x"), false)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn meeting_session_two_tracks_share_meeting_id_and_round_trip() {
        // Meeting Mode (v1.6): a meeting produces TWO recordings that share a
        // freshly-minted meeting_id and differ only by `track`. Both must
        // round-trip through insert/get/list, and a fresh single-track
        // recording must leave both columns NULL.
        let db = Catalog::open(Path::new("sqlite::memory:"))
            .await
            .expect("open db");

        let meeting_id = "meeting-abc123".to_string();
        let make = |track: &str| Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: format!("{track}.wav"),
            transcript: None,
            model: None,
            status: RecordingStatus::Transcribing,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: Some(meeting_id.clone()),
            meeting_name: None,
            track: Some(track.to_string()),
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            tags: vec![],
            speaker_names: vec![],
        };
        let mic = make("mic");
        let system = make("system");
        db.insert(&mic).await.expect("insert mic");
        db.insert(&system).await.expect("insert system");

        // Each row round-trips with its meeting_id + track intact.
        let got_mic = db.get(&mic.id).await.unwrap().unwrap();
        let got_sys = db.get(&system.id).await.unwrap().unwrap();
        assert_eq!(got_mic.meeting_id.as_deref(), Some("meeting-abc123"));
        assert_eq!(got_mic.track.as_deref(), Some("mic"));
        assert_eq!(got_sys.meeting_id.as_deref(), Some("meeting-abc123"));
        assert_eq!(got_sys.track.as_deref(), Some("system"));

        // The two recordings share one meeting_id.
        assert_eq!(got_mic.meeting_id, got_sys.meeting_id);

        // A normal single-track recording leaves both columns NULL.
        let solo = Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: "solo.wav".into(),
            transcript: None,
            model: None,
            status: RecordingStatus::Recording,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            tags: vec![],
            speaker_names: vec![],
        };
        db.insert(&solo).await.expect("insert solo");
        let got_solo = db.get(&solo.id).await.unwrap().unwrap();
        assert_eq!(got_solo.meeting_id, None);
        assert_eq!(got_solo.track, None);

        // Both meeting rows are visible via list().
        let all = db.list(&ListFilter::default()).await.unwrap();
        let with_session: Vec<_> = all
            .iter()
            .filter(|r| r.meeting_id.as_deref() == Some("meeting-abc123"))
            .collect();
        assert_eq!(with_session.len(), 2, "both meeting tracks must be listed");
    }

    // ── Named speakers ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn speaker_names_set_get_rename_and_clear() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // No names initially.
        assert!(db.speaker_names_for(&r.id).await.unwrap().is_empty());

        // Set two distinct speaker names; they come back ordered by index.
        db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();
        db.set_speaker_name(&r.id, 2, "Alex").await.unwrap();
        let names = db.speaker_names_for(&r.id).await.unwrap();
        assert_eq!(
            names,
            vec![
                SpeakerName {
                    speaker_label: 1,
                    name: "Sarah".into()
                },
                SpeakerName {
                    speaker_label: 2,
                    name: "Alex".into()
                },
            ]
        );

        // Re-setting the same label updates in place (upsert, not a duplicate row).
        db.set_speaker_name(&r.id, 1, "Sarah Connor").await.unwrap();
        let names = db.speaker_names_for(&r.id).await.unwrap();
        assert_eq!(names.len(), 2, "rename must not add a row");
        assert_eq!(names[0].name, "Sarah Connor");

        // Names are trimmed on the way in.
        db.set_speaker_name(&r.id, 2, "  Alex P.  ").await.unwrap();
        assert_eq!(
            db.speaker_names_for(&r.id).await.unwrap()[1].name,
            "Alex P."
        );

        // A blank/whitespace name clears the mapping (reverts to "Speaker N").
        db.set_speaker_name(&r.id, 1, "   ").await.unwrap();
        let names = db.speaker_names_for(&r.id).await.unwrap();
        assert_eq!(
            names,
            vec![SpeakerName {
                speaker_label: 2,
                name: "Alex P.".into()
            }],
            "clearing speaker 1 leaves only speaker 2"
        );
    }

    #[tokio::test]
    async fn speaker_names_are_populated_by_get_and_list() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();
        db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();

        // get() carries the speaker-name map (backs the detail view).
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(
            got.speaker_names,
            vec![SpeakerName {
                speaker_label: 1,
                name: "Sarah".into()
            }]
        );

        // list() carries it too.
        let listed = db.list(&ListFilter::default()).await.unwrap();
        let row = listed.iter().find(|x| x.id == r.id).unwrap();
        assert_eq!(row.speaker_names.len(), 1);
        assert_eq!(row.speaker_names[0].name, "Sarah");
    }

    #[tokio::test]
    async fn speaker_names_populated_per_track_by_list_by_meeting() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mic = embedded_recording(Some("m-1"));
        let sys = embedded_recording(Some("m-1"));
        db.insert(&mic).await.unwrap();
        db.insert(&sys).await.unwrap();
        // Each track keeps its own per-recording speaker names.
        db.set_speaker_name(&mic.id, 1, "Me").await.unwrap();
        db.set_speaker_name(&sys.id, 1, "Caller").await.unwrap();

        let tracks = db.list_by_meeting("m-1").await.unwrap();
        assert_eq!(tracks.len(), 2);
        for t in &tracks {
            let expected = if t.id == mic.id { "Me" } else { "Caller" };
            assert_eq!(
                t.speaker_names,
                vec![SpeakerName {
                    speaker_label: 1,
                    name: expected.into()
                }]
            );
        }
    }

    #[tokio::test]
    async fn speaker_names_cascade_deleted_with_recording() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();
        db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();

        db.delete(&r.id).await.unwrap();
        // The FK ON DELETE CASCADE must drop the orphaned name rows.
        assert!(
            db.speaker_names_for(&r.id).await.unwrap().is_empty(),
            "speaker names must be cascade-deleted with their recording"
        );
    }

    #[tokio::test]
    async fn retention_audio_only_keeps_rows_and_is_idempotent() {
        // delete_audio = true: the WAV path is returned for deletion and
        // blanked on the row, but the row itself (transcript, metadata)
        // SURVIVES — and a second sweep finds nothing left to reclaim.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mut r = embedded_recording(None);
        r.started_at = Local::now() - chrono::Duration::days(90);
        db.insert(&r).await.unwrap();

        let cfg = crate::config::RetentionConfig {
            max_age_days: Some(30),
            max_count: None,
            delete_audio: true,
        };
        let paths = db.apply_retention(&cfg).await.unwrap();
        assert_eq!(paths, vec!["x.wav".to_string()]);

        let row = db.get(&r.id).await.unwrap().expect("row must survive");
        assert_eq!(row.audio_path, "", "audio path blanked after reclaim");
        assert_eq!(row.transcript.as_deref(), Some("t"), "transcript kept");

        let again = db.apply_retention(&cfg).await.unwrap();
        assert!(again.is_empty(), "second sweep must be a no-op");
    }

    #[tokio::test]
    async fn retention_default_deletes_row_and_audio_together() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mut r = embedded_recording(None);
        r.started_at = Local::now() - chrono::Duration::days(90);
        db.insert(&r).await.unwrap();

        let cfg = crate::config::RetentionConfig {
            max_age_days: Some(30),
            max_count: None,
            delete_audio: false,
        };
        let paths = db.apply_retention(&cfg).await.unwrap();
        assert_eq!(paths.len(), 1);
        assert!(db.get(&r.id).await.unwrap().is_none(), "row deleted");
    }

    #[tokio::test]
    async fn clear_all_tag_suggestions_sweeps_every_recording() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        let b = embedded_recording(None);
        let c = embedded_recording(None); // never had suggestions
        db.insert(&a).await.unwrap();
        db.insert(&b).await.unwrap();
        db.insert(&c).await.unwrap();
        db.set_tag_suggestions(&a.id, &["alpha".into()])
            .await
            .unwrap();
        db.set_tag_suggestions(&b.id, &["beta".into(), "gamma".into()])
            .await
            .unwrap();

        let cleared = db.clear_all_tag_suggestions().await.unwrap();
        assert_eq!(cleared, 2, "only rows that HAD suggestions count");
        for id in [&a.id, &b.id, &c.id] {
            let rec = db.get(id).await.unwrap().unwrap();
            assert!(rec.tag_suggestions.is_empty());
        }
        // Sweep again: nothing left to clear.
        assert_eq!(db.clear_all_tag_suggestions().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn add_tag_is_case_insensitive() {
        // "Code" and "code" are the same tag: the second add must reuse the
        // first row (same id, its casing, its color) instead of minting a
        // byte-wise-unique duplicate.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let first = db.add_tag("Code", Some("#f00")).await.unwrap();
        let second = db.add_tag("code", None).await.unwrap();
        assert_eq!(first.id, second.id, "casing variants must reuse the tag");
        assert_eq!(second.name, "Code", "the first-created casing wins");
        assert_eq!(second.color.as_deref(), Some("#f00"), "existing color kept");
    }

    #[tokio::test]
    async fn segments_replace_round_trip_and_cascade() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // No segments yet is a normal (empty) state, not an error.
        assert!(db.segments_for(&r.id).await.unwrap().is_empty());

        let first = vec![
            TranscriptSegment {
                start_ms: 0,
                end_ms: 1200,
                text: "hello".into(),
                speaker: Some("1".into()),
            },
            TranscriptSegment {
                start_ms: 1200,
                end_ms: 2500,
                text: "hi there".into(),
                speaker: Some("2".into()),
            },
        ];
        db.replace_segments(&r.id, &first).await.unwrap();
        assert_eq!(db.segments_for(&r.id).await.unwrap(), first);

        // A retranscribe REPLACES the timeline — fewer rows must not leave
        // stale tail segments behind.
        let second = vec![TranscriptSegment {
            start_ms: 0,
            end_ms: 900,
            text: "rerun".into(),
            speaker: None,
        }];
        db.replace_segments(&r.id, &second).await.unwrap();
        assert_eq!(db.segments_for(&r.id).await.unwrap(), second);

        db.delete(&r.id).await.unwrap();
        assert!(
            db.segments_for(&r.id).await.unwrap().is_empty(),
            "segments must be cascade-deleted with their recording"
        );
    }
}
