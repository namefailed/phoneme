//! Structured task / action-item storage: the per-recording `tasks` child table
//! the LLM task-extraction step writes, plus the per-recording `tasks_model`
//! provenance column, the toggle-done mutation, and the browse-across-the-library
//! reads.
//!
//! Mirrors the entity-extraction path (`set_entities` / `set_entities_model` /
//! `list_entities` / `list_all_entities`) but adds the one thing entities don't
//! have: a mutable, user-owned `done` flag. Re-extraction must therefore PRESERVE
//! a `done` flag the user set rather than blindly DELETE+INSERT — see
//! [`Catalog::set_tasks`].

use super::*;
use std::collections::HashMap;

/// Fold task text to a match key for carrying `done` across re-extraction: lower
/// case, every run of non-alphanumerics (spaces, punctuation) collapsed to one
/// space, trimmed. So "Email Bob.", "email  bob", and "EMAIL BOB!" all key the
/// same — a minor reword keeps its checkbox. A full reword (different words)
/// still won't match, which is unavoidable without stable per-task ids.
fn normalize_task_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true; // skip leading separators
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

impl Catalog {
    /// Replace a recording's stored tasks, **preserving any `done` flag the user
    /// set** on a task whose text survives the re-extraction.
    ///
    /// Unlike [`Catalog::set_entities`] (which blindly DELETE+INSERTs, because
    /// entities are read-only), tasks carry user state: re-running the extraction
    /// step — or a re-transcribe that runs the `tasks` recipe step — must NOT
    /// silently un-check every task the user already completed. So in one
    /// transaction this reads the existing `(text, done)` map, deletes only the
    /// LLM-extracted rows (user-added `source='manual'` tasks survive a re-run),
    /// then inserts each new task carrying the prior `done` — matched on exact
    /// text first, then on a normalized key ([`normalize_task_text`]) so a minor
    /// reword (case / spacing / punctuation) keeps its tick. The
    /// `(recording_id, text)` UNIQUE is the storage key. An empty slice clears the
    /// extracted tasks (manual ones stay).
    pub async fn set_tasks(&self, id: &RecordingId, tasks: &[Task]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        // Snapshot the prior done-state, keyed by exact text, so a surviving task
        // keeps the checkbox the user ticked across re-extraction.
        let prior_rows = sqlx::query("SELECT text, done FROM tasks WHERE recording_id = ?")
            .bind(id.as_str())
            .fetch_all(&mut *tx)
            .await?;
        let mut prior_done: HashMap<String, bool> = HashMap::new();
        // Normalized-key map for catching minor rewordings; OR-accumulated so if
        // ANY prior task with that normalized text was done, the reworded one
        // inherits the tick.
        let mut prior_done_norm: HashMap<String, bool> = HashMap::new();
        for row in prior_rows {
            let text: String = row.try_get("text")?;
            let done: bool = row.try_get("done")?;
            *prior_done_norm
                .entry(normalize_task_text(&text))
                .or_insert(false) |= done;
            prior_done.insert(text, done);
        }
        // Only the extracted rows are replaced; user-added ('manual') tasks stay.
        sqlx::query("DELETE FROM tasks WHERE recording_id = ? AND source = 'llm'")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        for (i, t) in tasks.iter().enumerate() {
            // Exact match first, then the normalized fallback for minor rewords.
            let preserved = prior_done
                .get(&t.text)
                .copied()
                .or_else(|| prior_done_norm.get(&normalize_task_text(&t.text)).copied())
                .unwrap_or(false);
            let done = t.done || preserved;
            // Re-inserted rows are 'llm' and take their extraction order; manual
            // rows (kept above) keep their own sort_order. On a text collision
            // (a duplicate extraction, or a surviving manual task the LLM now
            // extracts verbatim) upsert rather than silently dropping the row:
            // the extracted values win, so the just-computed carried-over `done`
            // is never discarded. But a colliding 'manual' row STAYS 'manual' —
            // flipping it to 'llm' would mean the next re-extraction's DELETE
            // (above) silently removes a user-owned task, breaking this module's
            // promise that manual tasks survive a re-run. A plain INSERT (no OR
            // IGNORE) means a genuine constraint/FK error surfaces instead.
            sqlx::query(
                "INSERT INTO tasks (recording_id, text, due_hint, done, source, sort_order) \
                 VALUES (?, ?, ?, ?, 'llm', ?) \
                 ON CONFLICT(recording_id, text) DO UPDATE SET \
                 due_hint = excluded.due_hint, done = excluded.done, \
                 source = CASE WHEN tasks.source = 'manual' THEN 'manual' ELSE 'llm' END, \
                 sort_order = excluded.sort_order",
            )
            .bind(id.as_str())
            .bind(&t.text)
            .bind(t.due_hint.as_deref())
            .bind(done)
            .bind(i as i64)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Record which LLM model the task-extraction step used for this recording
    /// (the detail provenance line names it). Written once per run, after the
    /// tasks are stored. Mirrors [`Catalog::set_entities_model`].
    pub async fn set_tasks_model(&self, id: &RecordingId, model: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET tasks_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The tasks for one recording, open (not-done) first, then by the user's
    /// `sort_order`, then row id. Used to fill [`Recording::tasks`] (the N+1 child
    /// query, like [`Catalog::list_entities`]). Returns the row `id` so a single
    /// task can be toggled / edited / deleted unambiguously.
    pub async fn list_tasks(&self, id: &RecordingId) -> Result<Vec<Task>> {
        let rows = sqlx::query(
            "SELECT id, text, due_hint, done FROM tasks WHERE recording_id = ? ORDER BY done, sort_order, id",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Task {
                    id: r.try_get("id")?,
                    text: r.try_get("text")?,
                    due_hint: r.try_get("due_hint")?,
                    done: r.try_get("done")?,
                })
            })
            .collect()
    }

    /// Toggle (or set) one task's `done` flag by its row id, **scoped to its
    /// `recording_id`** — the one mutation the task feature adds (entities have no
    /// analogue). The UPDATE matches on both the row id and the recording, so a
    /// caller can never flip a task that belongs to a different recording than the
    /// one it named (which would also fire `TasksUpdated` for the wrong recording).
    /// A mismatched or unknown `(recording_id, task_id)` pair matches no row;
    /// callers that need to detect that check the affected row count (the handler
    /// maps 0 to `not_found`).
    pub async fn set_task_done(
        &self,
        recording_id: &RecordingId,
        task_id: i64,
        done: bool,
    ) -> Result<u64> {
        let res = sqlx::query("UPDATE tasks SET done = ? WHERE id = ? AND recording_id = ?")
            .bind(done)
            .bind(task_id)
            .bind(recording_id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// Every task across the whole library — the cross-recording "everything I
    /// have to do" list. Open tasks first, then newest recording first, carrying
    /// the `recording_id` + `title` so the UI/CLI can link back. When `only_open`
    /// is set, done tasks are dropped. The task counterpart of
    /// [`Catalog::list_all_entities`], but per-row (tasks have no `kind`/`value`
    /// to dedup across recordings).
    pub async fn list_all_tasks(&self, only_open: bool) -> Result<Vec<TaskWithRecording>> {
        // Newest recording first within each open/done group, then extraction
        // order within a recording. The optional `done = 0` filter is a static
        // clause (no bound param), injection-safe.
        let mut sql = String::from(
            "SELECT t.id, t.recording_id, t.text, t.due_hint, t.done, r.title \
             FROM tasks t JOIN recordings r ON r.id = t.recording_id",
        );
        if only_open {
            sql.push_str(" WHERE t.done = 0");
        }
        sql.push_str(" ORDER BY t.done, r.started_at DESC, t.sort_order, t.id");
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| {
                Ok(TaskWithRecording {
                    id: r.try_get("id")?,
                    recording_id: r.try_get("recording_id")?,
                    title: r.try_get("title")?,
                    text: r.try_get("text")?,
                    due_hint: r.try_get("due_hint")?,
                    done: r.try_get("done")?,
                })
            })
            .collect()
    }

    /// Library-wide task counts: open (not done) and total. The cheap badge
    /// counterpart of [`Catalog::list_all_tasks`] — computed in one SQL pass so
    /// the sidebar's Tasks badges don't fetch every row just to count them.
    pub async fn task_counts(&self) -> Result<crate::types::TaskCounts> {
        let row = sqlx::query(
            "SELECT
                COUNT(*) AS total,
                COALESCE(SUM(CASE WHEN done = 0 THEN 1 ELSE 0 END), 0) AS open
             FROM tasks",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(crate::types::TaskCounts {
            open: row.try_get("open")?,
            total: row.try_get("total")?,
        })
    }

    /// Add a user-created ('manual') task to a recording. Manual tasks survive
    /// re-extraction — [`Catalog::set_tasks`] only replaces the 'llm' rows.
    /// Appended after the current tasks (`sort_order = max + 1`). Returns the new id.
    pub async fn add_task(
        &self,
        recording_id: &RecordingId,
        text: &str,
        due_hint: Option<&str>,
    ) -> Result<i64> {
        let next: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM tasks WHERE recording_id = ?",
        )
        .bind(recording_id.as_str())
        .fetch_one(&self.pool)
        .await?;
        let res = sqlx::query(
            "INSERT INTO tasks (recording_id, text, due_hint, done, source, sort_order) \
             VALUES (?, ?, ?, 0, 'manual', ?)",
        )
        .bind(recording_id.as_str())
        .bind(text)
        .bind(due_hint)
        .bind(next)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// Edit one task's text (and optional due hint), scoped to its recording like
    /// [`Catalog::set_task_done`]. Returns the affected row count (0 = not found).
    pub async fn update_task(
        &self,
        recording_id: &RecordingId,
        task_id: i64,
        text: &str,
        due_hint: Option<&str>,
    ) -> Result<u64> {
        let res = sqlx::query(
            "UPDATE tasks SET text = ?, due_hint = ? WHERE id = ? AND recording_id = ?",
        )
        .bind(text)
        .bind(due_hint)
        .bind(task_id)
        .bind(recording_id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Delete one task, scoped to its recording. Returns the affected row count
    /// (0 = not found). Works for both 'llm' and 'manual' tasks.
    pub async fn delete_task(&self, recording_id: &RecordingId, task_id: i64) -> Result<u64> {
        let res = sqlx::query("DELETE FROM tasks WHERE id = ? AND recording_id = ?")
            .bind(task_id)
            .bind(recording_id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// Set the user's task order for a recording: each id in `ordered_ids` gets a
    /// `sort_order` equal to its position. Ids not in this recording are skipped
    /// (the UPDATE is scoped). One transaction, so the reorder is atomic.
    pub async fn reorder_tasks(
        &self,
        recording_id: &RecordingId,
        ordered_ids: &[i64],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (pos, task_id) in ordered_ids.iter().enumerate() {
            sqlx::query("UPDATE tasks SET sort_order = ? WHERE id = ? AND recording_id = ?")
                .bind(pos as i64)
                .bind(task_id)
                .bind(recording_id.as_str())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
