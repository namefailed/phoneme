use crate::error::Result;
use crate::id::RecordingId;
use crate::tags::Tag;
use crate::types::{ListFilter, Recording, RecordingStatus};
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
    /// `original_transcript` (the machine output) so the edit can be reverted.
    pub async fn update_user_transcript(&self, id: &RecordingId, transcript: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET transcript = ?, model = 'user-edit', updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(transcript)
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
        row.map(row_to_recording).transpose()
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
        // Populate tags for each recording (N+1 query; acceptable for desktop UI scale)
        for rec in &mut recs {
            rec.tags = self.tags_for(&rec.id).await.unwrap_or_default();
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
        rows.into_iter().map(row_to_recording).collect()
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

        // Age-based cleanup — delete everything older than max_age_days.
        if let Some(max_age) = cfg.max_age_days {
            let cutoff =
                chrono::Utc::now() - chrono::Duration::try_days(max_age as i64).unwrap_or_default();
            let cutoff_str = cutoff.to_rfc3339();
            let rows = sqlx::query(
                "SELECT id, audio_path FROM recordings \
                 WHERE started_at < ? AND status IN ('done','transcribe_failed','hook_failed')",
            )
            .bind(&cutoff_str)
            .fetch_all(&self.pool)
            .await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let audio_path: String = row.try_get("audio_path")?;
                sqlx::query("DELETE FROM recordings WHERE id = ?")
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                deleted_paths.push(audio_path);
            }
        }

        // Count-based cleanup — delete all but the most recent max_count.
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
                sqlx::query("DELETE FROM recordings WHERE id = ?")
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
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
        summary: row.try_get("summary").unwrap_or(None),
        summary_model: row.try_get("summary_model").unwrap_or(None),
        tags: Vec::new(),
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
            summary: None,
            summary_model: None,
            tags: vec![],
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
            summary: None,
            summary_model: None,
            tags: vec![],
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
            summary: None,
            summary_model: None,
            tags: vec![],
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
        assert_eq!(got.model.as_deref(), Some("user-edit"));
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
            summary: None,
            summary_model: None,
            tags: vec![],
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
            summary: None,
            summary_model: None,
            tags: vec![],
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
            summary: None,
            summary_model: None,
            tags: vec![],
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
}
