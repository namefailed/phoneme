//! Whole-meeting digests — one LLM synthesis across all of a meeting's tracks.
//!
//! Meetings aren't their own table (they're [`crate::Recording`] rows sharing a
//! `meeting_id`), so the digest lives in `meeting_digests`, keyed by `meeting_id`
//! (one row per meeting). This mirrors the per-recording summary
//! ([`Catalog::update_summary`] / the `summary` column) but at meeting scope.

use super::*;

impl Catalog {
    /// Store (or replace) the whole-meeting digest for `meeting_id`, along with
    /// the model that produced it. Upserts on the `meeting_id` primary key, so a
    /// regenerate overwrites the previous digest. Mirrors
    /// [`Catalog::update_summary`] at meeting scope.
    pub async fn update_meeting_digest(
        &self,
        meeting_id: &str,
        digest: &str,
        model: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO meeting_digests (meeting_id, digest, digest_model, updated_at)
               VALUES (?, ?, ?, datetime('now'))
               ON CONFLICT(meeting_id) DO UPDATE SET
                   digest = excluded.digest,
                   digest_model = excluded.digest_model,
                   updated_at = datetime('now')"#,
        )
        .bind(meeting_id)
        .bind(digest)
        .bind(model)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove a meeting's stored digest (if any). Called when the whole meeting
    /// is deleted, so the keyed-by-`meeting_id` row doesn't outlive its tracks —
    /// the table has no FK to `recordings` (a meeting isn't a single row) so the
    /// cleanup is explicit. A no-op when no digest was stored.
    pub async fn delete_meeting_digest(&self, meeting_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM meeting_digests WHERE meeting_id = ?")
            .bind(meeting_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// List every stored whole-meeting digest, one row per meeting. Used by the
    /// library backup export to capture digests — they live in this side table
    /// (keyed by `meeting_id`, no `Recording` DTO column), so a per-recording
    /// export would otherwise miss them. Ordered by `meeting_id` for a stable
    /// archive. An empty library yields an empty vec.
    pub async fn list_all_meeting_digests(&self) -> Result<Vec<MeetingDigest>> {
        let rows = sqlx::query(
            "SELECT meeting_id, digest, digest_model FROM meeting_digests ORDER BY meeting_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(MeetingDigest {
                    meeting_id: r.try_get("meeting_id")?,
                    digest: r.try_get("digest")?,
                    digest_model: r.try_get("digest_model")?,
                })
            })
            .collect()
    }

    /// Fetch the stored whole-meeting digest for `meeting_id`, or `None` when none
    /// has been generated yet. Read where the merged meeting is loaded (the daemon
    /// pairs this with [`Catalog::list_by_meeting`] for `ListMeeting`).
    pub async fn meeting_digest(&self, meeting_id: &str) -> Result<Option<MeetingDigest>> {
        let row = sqlx::query(
            "SELECT meeting_id, digest, digest_model FROM meeting_digests WHERE meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(MeetingDigest {
                meeting_id: r.try_get("meeting_id")?,
                digest: r.try_get("digest")?,
                digest_model: r.try_get("digest_model")?,
            })),
            None => Ok(None),
        }
    }
}
