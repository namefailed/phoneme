//! Saved searches and the AI-activity audit log.

use super::*;

impl Catalog {
    /// Persist one completed AI-activity session (a finished streaming LLM
    /// stage). Called by the daemon's `run_llm_stage` on success so the 🧠 popout
    /// can show the prompt + response after an app restart, not just live. Every
    /// insert prunes the table back to the newest `AI_ACTIVITY_KEEP` rows so it
    /// can't grow without bound; `created_at` is stored as RFC3339 UTC.
    pub async fn insert_ai_activity(
        &self,
        recording_id: &str,
        stage: &str,
        prompt: &str,
        response: &str,
    ) -> Result<()> {
        // Cap each field so a pathologically long transcript can't bloat the row.
        // Truncate on a char boundary (these fields are UTF-8 transcript text) so
        // a multi-byte character is never split, and append a marker so the popout
        // shows the text was clipped rather than silently ending mid-word.
        let cap = |s: &str| -> String {
            if s.chars().count() <= AI_ACTIVITY_FIELD_MAX_CHARS {
                return s.to_string();
            }
            let end = s
                .char_indices()
                .nth(AI_ACTIVITY_FIELD_MAX_CHARS)
                .map(|(i, _)| i)
                .unwrap_or(s.len());
            format!("{}… [truncated]", &s[..end])
        };
        let prompt = cap(prompt);
        let response = cap(response);
        sqlx::query(
            "INSERT INTO ai_activity (recording_id, stage, prompt, response, created_at) \
             VALUES (?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
        )
        .bind(recording_id)
        .bind(stage)
        .bind(&prompt)
        .bind(&response)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM ai_activity WHERE id NOT IN \
             (SELECT id FROM ai_activity ORDER BY id DESC LIMIT ?)",
        )
        .bind(AI_ACTIVITY_KEEP)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Recent AI-activity sessions, newest first. With `recording_id` set, only
    /// that recording's sessions; otherwise the whole library's recent activity.
    /// `limit` is clamped to `[1, AI_ACTIVITY_KEEP]`.
    pub async fn list_ai_activity(
        &self,
        recording_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AiActivityEntry>> {
        let limit = limit.clamp(1, AI_ACTIVITY_KEEP);
        let rows = match recording_id {
            Some(rid) => {
                sqlx::query(
                    "SELECT id, recording_id, stage, prompt, response, created_at \
                     FROM ai_activity WHERE recording_id = ? ORDER BY id DESC LIMIT ?",
                )
                .bind(rid)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT id, recording_id, stage, prompt, response, created_at \
                     FROM ai_activity ORDER BY id DESC LIMIT ?",
                )
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(AiActivityEntry {
                id: row.try_get("id")?,
                recording_id: row.try_get("recording_id")?,
                stage: row.try_get("stage")?,
                prompt: row.try_get("prompt")?,
                response: row.try_get("response")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    /// Record one in-place dictation in the opt-in re-grab ring buffer: the
    /// `text` that was typed at the cursor, plus the focused `app` exe stem when
    /// known. Called best-effort by the dictation core after the text has landed
    /// (only when `[in_place].keep_history` is on), so a past dictation can be
    /// re-inserted/re-copied later. Every insert prunes the table back to the
    /// newest `DICTATION_HISTORY_KEEP` rows so it can't grow without bound;
    /// `created_at` is stored as UTC. `char_count` records the dictation's real
    /// length even if an extreme outlier `text` is char-capped (mirroring the
    /// `ai_activity` cap idea).
    pub async fn insert_dictation_history(&self, text: &str, app: Option<&str>) -> Result<()> {
        // The real length, recorded before any char-cap so the UI still reports
        // the dictation's true size for an oversize outlier.
        let char_count = text.chars().count() as i64;
        // Cap the stored text on a char boundary (it is UTF-8) so a multi-byte
        // character is never split, with a marker so an extreme outlier reads as
        // clipped rather than silently ending mid-word — same approach as
        // `insert_ai_activity`.
        let stored = if text.chars().count() <= DICTATION_HISTORY_TEXT_MAX_CHARS {
            text.to_string()
        } else {
            let end = text
                .char_indices()
                .nth(DICTATION_HISTORY_TEXT_MAX_CHARS)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            format!("{}… [truncated]", &text[..end])
        };
        sqlx::query(
            "INSERT INTO dictation_history (text, char_count, app, created_at) \
             VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
        )
        .bind(&stored)
        .bind(char_count)
        .bind(app)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM dictation_history WHERE id NOT IN \
             (SELECT id FROM dictation_history ORDER BY id DESC LIMIT ?)",
        )
        .bind(DICTATION_HISTORY_KEEP)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Recent in-place dictations, newest first. `limit` is clamped to
    /// `[1, DICTATION_HISTORY_KEEP]`. Mirrors [`Self::list_ai_activity`].
    pub async fn list_dictation_history(&self, limit: i64) -> Result<Vec<DictationHistoryEntry>> {
        let limit = limit.clamp(1, DICTATION_HISTORY_KEEP);
        let rows = sqlx::query(
            "SELECT id, text, char_count, app, created_at \
             FROM dictation_history ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(DictationHistoryEntry {
                id: row.try_get("id")?,
                text: row.try_get("text")?,
                char_count: row.try_get("char_count")?,
                app: row.try_get("app")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    /// Fetch one stored dictation's text by id, for re-grab (re-insert/re-copy).
    /// `None` when no row has that id.
    pub async fn get_dictation_history(&self, id: i64) -> Result<Option<String>> {
        let row = sqlx::query("SELECT text FROM dictation_history WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => Ok(Some(row.try_get("text")?)),
            None => Ok(None),
        }
    }

    /// Delete one dictation-history row by id (unknown ids are a no-op). Returns
    /// whether a row was actually removed. Mirrors [`Self::delete_saved_search`].
    pub async fn delete_dictation_history(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM dictation_history WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Empty the whole dictation-history ring buffer ("clear all"). Returns how
    /// many rows were removed (mirrors `ClearFailed`'s `{removed:n}`).
    pub async fn clear_dictation_history(&self) -> Result<u64> {
        let res = sqlx::query("DELETE FROM dictation_history")
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// All saved searches, most-recently-updated first. The `filter_json` is
    /// returned verbatim for the frontend to deserialize.
    pub async fn list_saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let rows = sqlx::query(
            "SELECT id, name, filter_json FROM saved_searches \
             ORDER BY updated_at DESC, name COLLATE NOCASE ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(SavedSearch {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                filter_json: row.try_get("filter_json")?,
            });
        }
        Ok(out)
    }

    /// Insert or update a saved search by id. The frontend owns the by-name
    /// upsert and rename-conflict rules (it picks the id to write), so this is a
    /// plain by-id upsert — `created_at` is set once, `updated_at` on every write.
    pub async fn upsert_saved_search(&self, id: &str, name: &str, filter_json: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO saved_searches (id, name, filter_json, created_at, updated_at) \
             VALUES (?, ?, ?, datetime('now'), datetime('now')) \
             ON CONFLICT(id) DO UPDATE SET \
                 name = excluded.name, \
                 filter_json = excluded.filter_json, \
                 updated_at = datetime('now')",
        )
        .bind(id)
        .bind(name)
        .bind(filter_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a saved search by id (unknown ids are a no-op). Returns whether a
    /// row was actually removed.
    pub async fn delete_saved_search(&self, id: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM saved_searches WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Execute a stored saved search by id, server-side: look up its
    /// `filter_json`, parse it into a [`ListFilter`], and run the normal
    /// [`Self::list`] query, so a saved search produces the same recordings shape
    /// as a plain list without the frontend re-deriving the filter (S2).
    ///
    /// The persisted filter is the frontend's `UiFilter`
    /// ([`crate::SavedSearchFilter`]); the four-way `kind` and `tag_state` are
    /// mapped onto the daemon's `kind`/`favorite`/`in_place`/`tagged` exactly as
    /// the frontend's `toWireFilter` does, and UI-only display state (semantic /
    /// like-mode) is ignored — executing a saved search runs the list query, not a
    /// similarity or semantic search.
    ///
    /// The low-confidence toggle (when the saved filter carries it) maps to the
    /// daemon's numeric `low_confidence_below` using `low_confidence_threshold` —
    /// the live `[whisper].low_confidence_threshold` the daemon passes in, so a
    /// saved search captured with the Low-confidence filter actually filters
    /// server-side instead of running unfiltered.
    ///
    /// Errors: [`crate::Error::NotFound`] for an unknown id, and
    /// [`crate::Error::InvalidConfig`] when the stored `filter_json` won't parse
    /// (a hand-edit, a stale shape) — surfaced to the client verbatim rather than
    /// silently running the whole library.
    pub async fn run_saved_search(
        &self,
        id: &str,
        low_confidence_threshold: f32,
    ) -> Result<Vec<Recording>> {
        let row = sqlx::query("SELECT filter_json FROM saved_searches WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Err(crate::error::Error::NotFound {
                id: format!("saved search {id}"),
            });
        };
        let filter_json: String = row.try_get("filter_json")?;
        let filter =
            crate::SavedSearchFilter::parse_to_list_filter(&filter_json, low_confidence_threshold)?;
        self.list(&filter).await
    }
}
