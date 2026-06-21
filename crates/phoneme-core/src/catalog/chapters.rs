//! Auto-chapter storage: the per-recording `chapters` child table the LLM
//! auto-chapter enrichment step writes, plus the per-recording `chapters_model`
//! provenance column.
//!
//! Mirrors the transcript-segments shape (`replace_segments` / `segments_for`)
//! — a time-ranged child table replaced wholesale on each run — with the
//! provenance column convention the entities/summary steps established
//! (`set_entities_model`). A chapter is a `(start_ms, end_ms, title, summary)`
//! span; the daemon's `parse_chapters` anchors the boundaries to real segment
//! starts before they land here, so a stored row always lines up with the audio.

use super::*;
use crate::types::Chapter;

impl Catalog {
    /// Replace a recording's stored chapters wholesale: delete the existing rows,
    /// then bulk-insert `chapters` in order, in one transaction. An empty slice
    /// clears them. Mirrors [`Catalog::replace_segments`] — one transaction so a
    /// partial write can't leave a recording with a mix of old and new chapters.
    ///
    /// `idx` is the slice position, so the rows read back in the order passed in
    /// (the daemon passes them already sorted by `start_ms`).
    pub async fn replace_chapters(&self, id: &RecordingId, chapters: &[Chapter]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM chapters WHERE recording_id = ?")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        for (idx, c) in chapters.iter().enumerate() {
            sqlx::query(
                "INSERT INTO chapters (recording_id, idx, start_ms, end_ms, title, summary) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id.as_str())
            .bind(idx as i64)
            .bind(c.start_ms)
            .bind(c.end_ms)
            .bind(&c.title)
            .bind(&c.summary)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A recording's chapters in `idx` (chronological) order. Empty when the
    /// recording has no timing to chapter or the auto-chapter step never ran —
    /// callers must treat "no chapters" as a normal state, not an error (mirrors
    /// [`Catalog::segments_for`]).
    pub async fn chapters_for(&self, id: &RecordingId) -> Result<Vec<Chapter>> {
        let rows = sqlx::query(
            "SELECT start_ms, end_ms, title, summary FROM chapters \
             WHERE recording_id = ? ORDER BY idx",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Chapter {
                    start_ms: r.try_get("start_ms")?,
                    end_ms: r.try_get("end_ms")?,
                    title: r.try_get("title")?,
                    summary: r.try_get("summary")?,
                })
            })
            .collect()
    }

    /// Record which LLM model the auto-chapter step used for this recording (the
    /// detail provenance line names it). Written once per run, before the chapters
    /// are stored. Mirrors [`Catalog::set_entities_model`].
    pub async fn set_chapters_model(&self, id: &RecordingId, model: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET chapters_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
