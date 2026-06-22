//! Structured entity storage: the per-recording `entities` child table the
//! LLM entity-extraction step writes, plus the per-recording `entities_model`
//! provenance column and the browse-across-the-library reads.
//!
//! Mirrors the auto-tag suggestion path (`set_tag_suggestions` / `set_tag_model`
//! / `list_all_tags`) but keeps entities in their own typed child table rather
//! than a JSON column, so they can be grouped by `kind` and browsed by `value`.

use super::*;

impl Catalog {
    /// Replace a recording's LLM-extracted entities: delete the existing `'llm'`
    /// rows (user-curated `source='manual'` entities survive a re-run), then
    /// bulk-insert `entities` as `'llm'` (de-duplicated by the table's
    /// `(recording_id, kind, value)` UNIQUE via `INSERT OR IGNORE`). An empty
    /// slice clears the extracted entities (manual ones stay). One transaction so
    /// a partial write can't leave a recording with a mix of old and new.
    pub async fn set_entities(&self, id: &RecordingId, entities: &[Entity]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM entities WHERE recording_id = ? AND source = 'llm'")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        for e in entities {
            sqlx::query(
                "INSERT OR IGNORE INTO entities (recording_id, kind, value, source) VALUES (?, ?, ?, 'llm')",
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

    /// Add a user-curated ('manual') entity to a recording; it survives later
    /// re-extraction. `INSERT OR IGNORE`, so re-adding an existing `(kind, value)`
    /// is a no-op. Returns the affected row count (0 = already present).
    pub async fn add_entity(&self, id: &RecordingId, kind: &str, value: &str) -> Result<u64> {
        let res = sqlx::query(
            "INSERT OR IGNORE INTO entities (recording_id, kind, value, source) VALUES (?, ?, ?, 'manual')",
        )
        .bind(id.as_str())
        .bind(kind)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Edit one entity in place (fix a wrong kind/value), scoped to its recording
    /// and keyed by its current `(kind, value)`. Marks it `'manual'` so the fix
    /// survives re-extraction. `UPDATE OR IGNORE` so renaming onto an entity the
    /// recording already has is a no-op rather than a UNIQUE violation. Returns
    /// the affected row count (0 = not found / would-collide).
    pub async fn update_entity(
        &self,
        id: &RecordingId,
        kind: &str,
        value: &str,
        new_kind: &str,
        new_value: &str,
    ) -> Result<u64> {
        let res = sqlx::query(
            "UPDATE OR IGNORE entities SET kind = ?, value = ?, source = 'manual' \
             WHERE recording_id = ? AND kind = ? AND value = ?",
        )
        .bind(new_kind)
        .bind(new_value)
        .bind(id.as_str())
        .bind(kind)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Delete one entity from a recording, keyed by `(kind, value)`. Returns the
    /// affected row count (0 = not found). Works for both 'llm' and 'manual' rows.
    pub async fn delete_entity(&self, id: &RecordingId, kind: &str, value: &str) -> Result<u64> {
        let res =
            sqlx::query("DELETE FROM entities WHERE recording_id = ? AND kind = ? AND value = ?")
                .bind(id.as_str())
                .bind(kind)
                .bind(value)
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected())
    }

    /// Library-wide merge: fold every `from_values` entity of `kind` into
    /// `to_value` (e.g. "ACME", "acme corp" → "Acme Corp"). Across every recording
    /// it renames the variant to the canonical value and marks it `'manual'` so the
    /// merge sticks through re-extraction; a recording that already has both keeps
    /// one (the `UPDATE OR IGNORE` skips the dup, then the leftover variant row is
    /// deleted). One transaction. Returns how many rows were renamed.
    pub async fn merge_entities(
        &self,
        kind: &str,
        from_values: &[String],
        to_value: &str,
    ) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let mut renamed = 0u64;
        for from in from_values {
            if from == to_value {
                continue;
            }
            let r = sqlx::query(
                "UPDATE OR IGNORE entities SET value = ?, source = 'manual' WHERE kind = ? AND value = ?",
            )
            .bind(to_value)
            .bind(kind)
            .bind(from)
            .execute(&mut *tx)
            .await?;
            renamed += r.rows_affected();
            // Drop any variant rows the OR IGNORE skipped (recording already had
            // the canonical value), so the old name disappears everywhere.
            sqlx::query("DELETE FROM entities WHERE kind = ? AND value = ?")
                .bind(kind)
                .bind(from)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(renamed)
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

    /// The cross-recording entity facet: every distinct `(kind, value)` across
    /// the library with its recording count, ordered by kind then value. The
    /// entity counterpart of the tag facet — [`Catalog::list_all_tags`] plus
    /// [`Catalog::tag_usage_counts`] in one pass — powering the sidebar's
    /// browse-by-entity surface (group by `kind`, each `value` a filter row
    /// showing its `count`).
    ///
    /// The `(recording_id, kind, value)` UNIQUE means each recording contributes
    /// at most one row per `(kind, value)`, so `COUNT(*)` after grouping is the
    /// number of recordings that mention it (not raw mentions). De-duplicated
    /// across recordings via the GROUP BY.
    pub async fn entity_facets(&self) -> Result<Vec<EntityFacet>> {
        let rows = sqlx::query(
            "SELECT kind, value, COUNT(*) AS cnt FROM entities \
             GROUP BY kind, value ORDER BY kind, value",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(EntityFacet {
                    kind: r.try_get("kind")?,
                    value: r.try_get("value")?,
                    count: r.try_get("cnt")?,
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
