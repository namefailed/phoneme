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

impl Catalog {
    /// Replace a recording's stored tasks, **preserving any `done` flag the user
    /// set** on a task whose text survives the re-extraction.
    ///
    /// Unlike [`Catalog::set_entities`] (which blindly DELETE+INSERTs, because
    /// entities are read-only), tasks carry user state: re-running the extraction
    /// step — or a re-transcribe that runs the `tasks` recipe step — must NOT
    /// silently un-check every task the user already completed. So in one
    /// transaction this reads the existing `(text, done)` map, deletes all rows,
    /// then inserts each new task with `done = old_done_for_same_text OR
    /// task.done`. The `(recording_id, text)` UNIQUE is the merge key (the same
    /// text-keyed limitation entities have — a reworded task reappears unchecked).
    /// An empty slice clears them.
    pub async fn set_tasks(&self, id: &RecordingId, tasks: &[Task]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        // Snapshot the prior done-state, keyed by exact text, so a surviving task
        // keeps the checkbox the user ticked across re-extraction.
        let prior_rows = sqlx::query("SELECT text, done FROM tasks WHERE recording_id = ?")
            .bind(id.as_str())
            .fetch_all(&mut *tx)
            .await?;
        let mut prior_done: HashMap<String, bool> = HashMap::new();
        for row in prior_rows {
            let text: String = row.try_get("text")?;
            let done: bool = row.try_get("done")?;
            prior_done.insert(text, done);
        }
        sqlx::query("DELETE FROM tasks WHERE recording_id = ?")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        for t in tasks {
            let done = t.done || prior_done.get(&t.text).copied().unwrap_or(false);
            sqlx::query(
                "INSERT OR IGNORE INTO tasks (recording_id, text, due_hint, done) VALUES (?, ?, ?, ?)",
            )
            .bind(id.as_str())
            .bind(&t.text)
            .bind(t.due_hint.as_deref())
            .bind(done)
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

    /// The tasks extracted for one recording, open (not-done) first, then by row
    /// id (extraction order). Used to fill [`Recording::tasks`] (the N+1 child
    /// query, like [`Catalog::list_entities`]). Returns the row `id` so a single
    /// task can be toggled done unambiguously.
    pub async fn list_tasks(&self, id: &RecordingId) -> Result<Vec<Task>> {
        let rows = sqlx::query(
            "SELECT id, text, due_hint, done FROM tasks WHERE recording_id = ? ORDER BY done, id",
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

    /// Toggle (or set) one task's `done` flag by its row id — the one mutation the
    /// task feature adds (entities have no analogue). No-op when `task_id` is
    /// unknown (the `UPDATE` simply matches no row); callers that need to detect a
    /// missing task check the affected row count.
    pub async fn set_task_done(&self, task_id: i64, done: bool) -> Result<u64> {
        let res = sqlx::query("UPDATE tasks SET done = ? WHERE id = ?")
            .bind(done)
            .bind(task_id)
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
        sql.push_str(" ORDER BY t.done, r.started_at DESC, t.id");
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
}
