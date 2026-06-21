//! Period digests — one LLM rollup across every recording in a date window.
//!
//! A period spans many independent recordings (selected by a `since..until`
//! range), so it has no parent row to hang off — unlike the per-recording
//! `summary` and unlike [`MeetingDigest`](crate::MeetingDigest), which is keyed
//! by `meeting_id`. It lives in `period_digests`, keyed by a stable `key`
//! derived from the canonical range bounds (one row per range), so re-running
//! the same window upserts in place. This mirrors `meeting_digests.rs` at period
//! scope.

use super::*;
use crate::types::PeriodDigest;

impl Catalog {
    /// Store (or replace) the period digest for `key`, along with its label,
    /// range bounds, model, and how many recordings were rolled up. Upserts on
    /// the `key` primary key, so a regenerate of the same window overwrites the
    /// previous digest. `since`/`until` are stored as RFC3339 text. Mirrors
    /// [`Catalog::update_meeting_digest`] at period scope.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_period_digest(
        &self,
        key: &str,
        label: &str,
        since: DateTime<Local>,
        until: DateTime<Local>,
        digest: &str,
        model: Option<&str>,
        source_count: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO period_digests
                   (key, label, since, until, digest, digest_model, source_count, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'))
               ON CONFLICT(key) DO UPDATE SET
                   label = excluded.label,
                   since = excluded.since,
                   until = excluded.until,
                   digest = excluded.digest,
                   digest_model = excluded.digest_model,
                   source_count = excluded.source_count,
                   updated_at = datetime('now')"#,
        )
        .bind(key)
        .bind(label)
        .bind(since.to_rfc3339())
        .bind(until.to_rfc3339())
        .bind(digest)
        .bind(model)
        .bind(source_count)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove a stored period digest by `key` (if any). A no-op when none was
    /// stored. Period digests have no parent row, so cleanup is explicit (there
    /// is no cascade); a re-run of the same range simply upserts over it.
    pub async fn delete_period_digest(&self, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM period_digests WHERE key = ?")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// List every stored period digest, newest range first (`since DESC`). Used
    /// by the library-backup export to capture them — they live in this side
    /// table (no `Recording` DTO column), so a per-recording export would
    /// otherwise miss them. An empty library yields an empty vec.
    pub async fn list_all_period_digests(&self) -> Result<Vec<PeriodDigest>> {
        let rows = sqlx::query(
            "SELECT key, label, since, until, digest, digest_model, source_count \
             FROM period_digests ORDER BY since DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_period_digest).collect()
    }

    /// Fetch the stored period digest for `key`, or `None` when none has been
    /// generated for that range yet. The `key` is the stable range id the daemon
    /// derived from the canonical bounds.
    pub async fn period_digest(&self, key: &str) -> Result<Option<PeriodDigest>> {
        let row = sqlx::query(
            "SELECT key, label, since, until, digest, digest_model, source_count \
             FROM period_digests WHERE key = ?",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(row_to_period_digest(r)?)),
            None => Ok(None),
        }
    }
}

/// Decode one `period_digests` row into a [`PeriodDigest`]. `since`/`until` are
/// stored as RFC3339 text and parsed back to `DateTime<Local>` via the shared
/// [`parse_dt`] helper.
fn row_to_period_digest(r: sqlx::sqlite::SqliteRow) -> Result<PeriodDigest> {
    let since: String = r.try_get("since")?;
    let until: String = r.try_get("until")?;
    Ok(PeriodDigest {
        key: r.try_get("key")?,
        label: r.try_get("label")?,
        since: parse_dt(&since)?,
        until: parse_dt(&until)?,
        digest: r.try_get("digest")?,
        digest_model: r.try_get("digest_model")?,
        source_count: r.try_get("source_count")?,
    })
}
