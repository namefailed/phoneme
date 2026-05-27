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
            r#"INSERT INTO recordings
                 (id, started_at, duration_ms, audio_path, transcript, model, status,
                  error_kind, error_message, hook_command, hook_exit_code,
                  hook_duration_ms, transcribed_at, hook_ran_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
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
        .bind(r.transcribed_at.map(|t| t.to_rfc3339()))
        .bind(r.hook_ran_at.map(|t| t.to_rfc3339()))
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

    /// Update just the duration column. Used when the recorder finalizes a
    /// WAV and we know the captured length.
    pub async fn update_duration(&self, id: &RecordingId, duration_ms: i64) -> Result<()> {
        sqlx::query(
            "UPDATE recordings SET duration_ms = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(duration_ms)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_transcript(
        &self,
        id: &RecordingId,
        transcript: &str,
        model: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET transcript = ?, model = ?,
                   transcribed_at = datetime('now'), updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(transcript)
        .bind(model)
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
        sql.push_str(" ORDER BY recordings.started_at DESC, recordings.id DESC");
        if let Some(n) = filter.limit {
            sql.push_str(&format!(" LIMIT {n}"));
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
        let rows = q.fetch_all(&self.pool).await?;
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

    pub async fn delete_tag(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
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
            since: None,
            status: None,
            search: None,
            tag_id: None,
        };
        let list = db.list(&filter).await.expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id.as_str(), r.id.as_str());
    }
}
