//! Tag CRUD, attach/detach, usage counts, merge, and retention sweeps.

use super::*;

impl Catalog {
    /// Tags attached to at least one recording, name-sorted. Powers the filter
    /// dropdown and tag autocomplete — orphaned tags are excluded (see
    /// [`Catalog::list_all_tags`] for the unfiltered set).
    pub async fn list_tags(&self) -> Result<Vec<Tag>> {
        // Only tags attached to at least one recording; orphaned tags would
        // otherwise clutter the filter dropdown and tag autocomplete.
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

    /// Create a tag named `name`, or return the existing one (matching is
    /// case-insensitive, so adding "code" when "Code" exists reuses it, colour
    /// and links intact).
    pub async fn add_tag(&self, name: &str, color: Option<&str>) -> Result<Tag> {
        // Tags are case-insensitively unique at the application level: "Code" and
        // "code" are the same tag, so adding either reuses the existing row,
        // keeping its color and recording links (the first-created casing wins).
        // The UNIQUE index on `name` is byte-wise, so this lookup is what guards
        // the insert. COLLATE NOCASE is ASCII-only, which covers the realistic
        // duplicate ("Test"/"test") without rewriting non-ASCII tag names.
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

    /// Rename and/or recolour an existing tag, returning the updated record.
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

    /// Delete a tag entirely; cascading foreign keys detach it from every
    /// recording it was on.
    pub async fn delete_tag(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every recording id currently in the catalog — for de-duping a disk
    /// re-import (`ReimportFromDisk`) against rows that already exist. Ids that
    /// somehow fail the canonical shape check are skipped rather than panicking.
    pub async fn all_ids(&self) -> Result<Vec<RecordingId>> {
        let rows = sqlx::query("SELECT id FROM recordings")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let s: String = r.try_get("id").ok()?;
                RecordingId::parse(s)
            })
            .collect())
    }

    /// Every tag, including ones not attached to any recording. Used by the Tag
    /// Manager settings UI.
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

    /// Attach a tag to a recording (idempotent — attaching an already-attached
    /// tag is a no-op).
    pub async fn attach_tag(&self, recording_id: &RecordingId, tag_id: i64) -> Result<()> {
        sqlx::query("INSERT OR IGNORE INTO recording_tags (recording_id, tag_id) VALUES (?, ?)")
            .bind(recording_id.as_str())
            .bind(tag_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Detach a tag from a recording (the tag itself is left intact).
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

    /// Full-corpus recording counts per Library kind (all / single / meeting /
    /// in-place / favorite / pinned), computed in one SQL pass. Powers the
    /// sidebar's Library count badges (the GUI counterpart of
    /// `tag_usage_counts`).
    pub async fn kind_counts(&self) -> Result<crate::types::KindCounts> {
        let row = sqlx::query(
            "SELECT
                COUNT(*) AS all_count,
                COALESCE(SUM(CASE WHEN meeting_id IS NULL THEN 1 ELSE 0 END), 0) AS single_count,
                COALESCE(SUM(CASE WHEN meeting_id IS NOT NULL THEN 1 ELSE 0 END), 0) AS meeting_count,
                COALESCE(SUM(CASE WHEN in_place = 1 THEN 1 ELSE 0 END), 0) AS in_place_count,
                COALESCE(SUM(CASE WHEN favorite = 1 THEN 1 ELSE 0 END), 0) AS favorite_count,
                COALESCE(SUM(CASE WHEN pinned = 1 THEN 1 ELSE 0 END), 0) AS pinned_count,
                (SELECT COUNT(DISTINCT recording_id) FROM recording_tags) AS tagged_count,
                (COUNT(*) - (SELECT COUNT(DISTINCT recording_id) FROM recording_tags)) AS untagged_count
             FROM recordings",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(crate::types::KindCounts {
            all: row.try_get("all_count")?,
            single: row.try_get("single_count")?,
            meeting: row.try_get("meeting_count")?,
            in_place: row.try_get("in_place_count")?,
            favorite: row.try_get("favorite_count")?,
            pinned: row.try_get("pinned_count")?,
            tagged: row.try_get("tagged_count")?,
            untagged: row.try_get("untagged_count")?,
        })
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
    /// Only terminal-state recordings (done / failed / cancelled) are eligible;
    /// in-progress recordings are always kept, whatever their age or count.
    pub async fn apply_retention(
        &self,
        cfg: &crate::config::RetentionConfig,
    ) -> Result<Vec<String>> {
        let mut deleted_paths: Vec<String> = Vec::new();
        // Tracks whether any row was hard-deleted (not audio-only). A hard
        // delete cascade-drops that recording's embeddings, so the warm cache
        // must be dropped or deleted vectors keep surfacing in search.
        let mut hard_deleted = false;
        // Named voices that lose a sample to a hard delete, collected as we go —
        // before each delete cascades the voiceprint away — so their centroids and
        // counts can be recomputed once at the end, mirroring [`Self::delete`]
        // (audit M1). A `HashSet` dedupes voices touched by several recordings.
        let mut affected_voices: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // `delete_audio = true` is the disk-saver mode: the catalog row stays (the
        // transcript stays searchable), only the WAV goes. The row's audio_path is
        // blanked so the UI doesn't offer a dead player and so a later sweep won't
        // re-process the row. `false` (the default) deletes row and audio together.
        let audio_only = cfg.delete_audio;

        // Age-based cleanup — everything older than max_age_days.
        if let Some(max_age) = cfg.max_age_days {
            let cutoff =
                chrono::Utc::now() - chrono::Duration::try_days(max_age as i64).unwrap_or_default();
            let cutoff_str = cutoff.to_rfc3339();
            let rows = sqlx::query(&format!(
                "SELECT id, audio_path FROM recordings \
                 WHERE started_at < ? \
                 AND status IN ({}) \
                 AND audio_path != ''",
                RecordingStatus::terminal_sql_list()
            ))
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
                    // Capture the named voices losing a sample before the cascade
                    // removes this recording's voiceprints (audit M1).
                    let voices: Vec<String> = sqlx::query_scalar(
                        "SELECT DISTINCT named_voice_id FROM speaker_voiceprints \
                         WHERE recording_id = ? AND named_voice_id IS NOT NULL",
                    )
                    .bind(&id)
                    .fetch_all(&self.pool)
                    .await?;
                    affected_voices.extend(voices);
                    sqlx::query("DELETE FROM recordings WHERE id = ?")
                        .bind(&id)
                        .execute(&self.pool)
                        .await?;
                    hard_deleted = true;
                }
                deleted_paths.push(audio_path);
            }
        }

        // Count-based cleanup — all but the most recent max_count. In audio-only
        // mode the ranking still counts every terminal row: the rows are kept, so
        // "the most recent N" has to mean recordings, not files. The audio_path
        // filter above and below only stops re-processing rows whose audio is
        // already gone.
        if let Some(max_count) = cfg.max_count {
            let rows = sqlx::query(&format!(
                "SELECT id, audio_path FROM recordings \
                 WHERE status IN ({}) \
                 ORDER BY started_at DESC, id DESC \
                 LIMIT -1 OFFSET ?",
                RecordingStatus::terminal_sql_list()
            ))
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
                    // Capture the named voices losing a sample before the cascade
                    // removes this recording's voiceprints (audit M1).
                    let voices: Vec<String> = sqlx::query_scalar(
                        "SELECT DISTINCT named_voice_id FROM speaker_voiceprints \
                         WHERE recording_id = ? AND named_voice_id IS NOT NULL",
                    )
                    .bind(&id)
                    .fetch_all(&self.pool)
                    .await?;
                    affected_voices.extend(voices);
                    sqlx::query("DELETE FROM recordings WHERE id = ?")
                        .bind(&id)
                        .execute(&self.pool)
                        .await?;
                    hard_deleted = true;
                }
                deleted_paths.push(audio_path);
            }
        }

        // A hard delete cascade-drops the recordings' embeddings; drop the warm
        // snapshot so deleted vectors stop surfacing in semantic search. (The
        // audio-only path keeps the rows and embeddings, so it needs no drop.)
        if hard_deleted {
            self.invalidate_embedding_cache();
        }
        // ...and recompute any named voice that just lost a sample so the Speaker
        // Library's cached centroids and counts stay accurate (audit M1).
        for nid in affected_voices {
            self.recompute_named_centroid(&nid).await?;
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

        // Items older than cutoff_now are already deleted, or being deleted now.
        let cutoff_now =
            chrono::Utc::now() - chrono::Duration::try_days(max_age as i64).unwrap_or_default();
        // Items older than cutoff_future fall due over the next `hours_ahead` hours.
        let cutoff_future =
            cutoff_now + chrono::Duration::try_hours(hours_ahead as i64).unwrap_or_default();

        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM recordings \
             WHERE started_at >= ? AND started_at < ? \
             AND status IN ({})",
            RecordingStatus::terminal_sql_list()
        ))
        .bind(cutoff_now.to_rfc3339())
        .bind(cutoff_future.to_rfc3339())
        .fetch_one(&self.pool)
        .await?;

        Ok(count as u32)
    }

    /// The tags attached to one recording, name-sorted. Used to fill
    /// `Recording::tags` and back the detail view.
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
