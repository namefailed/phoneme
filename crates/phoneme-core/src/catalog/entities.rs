//! Structured entity storage: the per-recording `entities` child table the
//! LLM entity-extraction step writes, plus the per-recording `entities_model`
//! provenance column and the browse-across-the-library reads.
//!
//! Mirrors the auto-tag suggestion path (`set_tag_suggestions` / `set_tag_model`
//! / `list_all_tags`) but keeps entities in their own typed child table rather
//! than a JSON column, so they can be grouped by `kind` and browsed by `value`.

use super::*;

impl Catalog {
    /// Replace a recording's stored entities wholesale: delete the existing rows,
    /// then bulk-insert `entities` (de-duplicated by the table's `(recording_id,
    /// kind, value)` UNIQUE via `INSERT OR IGNORE`). An empty slice clears them.
    /// Mirrors [`Catalog::set_tag_suggestions`] — one transaction so a partial
    /// write can't leave a recording with a mix of old and new entities.
    pub async fn set_entities(&self, id: &RecordingId, entities: &[Entity]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM entities WHERE recording_id = ?")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        for e in entities {
            sqlx::query(
                "INSERT OR IGNORE INTO entities (recording_id, kind, value) VALUES (?, ?, ?)",
            )
            .bind(id.as_str())
            .bind(&e.kind)
            .bind(&e.value)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Record which LLM model the entity-extraction step used for this recording
    /// (the detail provenance line names it). Written once per run, before the
    /// entities are stored. Mirrors [`Catalog::set_tag_model`].
    pub async fn set_entities_model(&self, id: &RecordingId, model: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET entities_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The entities extracted for one recording, ordered by kind then value so a
    /// per-recording fetch returns them grouped. Used to fill
    /// [`Recording::entities`] (the N+1 child query, like [`Catalog::tags_for`]).
    pub async fn list_entities(&self, id: &RecordingId) -> Result<Vec<Entity>> {
        let rows = sqlx::query(
            "SELECT kind, value FROM entities WHERE recording_id = ? ORDER BY kind, value",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Entity {
                    kind: r.try_get("kind")?,
                    value: r.try_get("value")?,
                })
            })
            .collect()
    }

    /// Every distinct entity across the whole library, ordered by kind then
    /// value. Powers the browse-entities surface (the entity counterpart of
    /// [`Catalog::list_all_tags`]). De-duplicated across recordings — the same
    /// `(kind, value)` mentioned in several recordings appears once.
    pub async fn list_all_entities(&self) -> Result<Vec<Entity>> {
        let rows = sqlx::query("SELECT DISTINCT kind, value FROM entities ORDER BY kind, value")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Entity {
                    kind: r.try_get("kind")?,
                    value: r.try_get("value")?,
                })
            })
            .collect()
    }

    /// Every distinct entity of one `kind` across the library, value-sorted —
    /// the by-kind slice of [`Catalog::list_all_entities`] for a browse filter
    /// (e.g. "show every person"). De-duplicated across recordings.
    pub async fn entities_by_kind(&self, kind: &str) -> Result<Vec<Entity>> {
        let rows =
            sqlx::query("SELECT DISTINCT kind, value FROM entities WHERE kind = ? ORDER BY value")
                .bind(kind)
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Entity {
                    kind: r.try_get("kind")?,
                    value: r.try_get("value")?,
                })
            })
            .collect()
    }
}
