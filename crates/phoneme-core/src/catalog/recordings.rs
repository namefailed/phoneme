//! Recording row CRUD, transcript/metadata updates, listing, and lifecycle.

use super::*;

/// How many ids one batched `WHERE recording_id IN (…)` child query binds at
/// once. Kept well under SQLite's older 999 bound-parameter cap so an
/// unpaginated `list()` over a large corpus splits into a handful of queries
/// rather than failing — a paginated library page is a single chunk.
const IN_CHUNK: usize = 900;

/// Build a `?, ?, …` placeholder list of length `n` for an `IN (…)` clause. `n`
/// is always a chunk length the caller controls (never user input), so the
/// generated text is safe to interpolate; the values themselves are still bound.
fn in_placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// A bound value for the dynamic recordings-`list` filter, replayed in WHERE
/// order by both [`Catalog::list`] (full rows) and [`Catalog::list_ids`]
/// (id-only). Keeping the SQL + bind building in one place is what stops the two
/// queries from drifting.
enum ListBind {
    Text(String),
    F32(f32),
}

/// Build the part of the recordings query after `FROM recordings` — the optional
/// tag JOIN, the dynamic WHERE, ORDER BY, and LIMIT/OFFSET — plus the bind values
/// in WHERE order. Shared by [`Catalog::list`] and [`Catalog::list_ids`] so a
/// filter behaves identically whether the caller fetches whole rows or just ids.
fn list_query_suffix(filter: &ListFilter) -> (String, Vec<ListBind>) {
    let mut sql = String::new();

    let mut fts_query = None;
    let mut tag_search_query = None;
    let mut model_search_query = None;

    if let Some(q) = filter.search.as_deref() {
        let sanitized = sanitize_fts5_query(q);
        if !sanitized.is_empty() {
            fts_query = Some(sanitized);
            let like = format!("%{}%", q);
            tag_search_query = Some(like.clone());
            // The same substring also matches any step's model name, so a search
            // like "large-v3" or "gemma3:4b" finds everything that model ran on.
            model_search_query = Some(like);
        }
    }

    if filter.tag_id.is_some() {
        sql.push_str(" JOIN recording_tags rt ON rt.recording_id = recordings.id");
    }

    sql.push_str(" WHERE 1=1");

    if fts_query.is_some() {
        sql.push_str(" AND (recordings.rowid IN (SELECT rowid FROM recordings_fts WHERE transcript MATCH ?) OR recordings.id IN (SELECT recording_id FROM recording_tags rts JOIN tags ts ON ts.id = rts.tag_id WHERE ts.name LIKE ?) OR recordings.model LIKE ? OR recordings.cleanup_model LIKE ? OR recordings.summary_model LIKE ? OR recordings.title_model LIKE ? OR recordings.tag_model LIKE ? OR recordings.diarization_model LIKE ?)");
    }
    if let Some(tag_id) = filter.tag_id {
        // `tag_id` is an `i64`, so formatting it directly is injection-safe — an
        // integer can't carry SQL. Same rationale as the `u32` LIMIT/OFFSET below.
        sql.push_str(&format!(" AND rt.tag_id = {tag_id}"));
    }
    if filter.status.is_some() {
        sql.push_str(" AND recordings.status = ?");
    }
    match filter.kind {
        Some(crate::types::ListKind::Single) => {
            sql.push_str(" AND recordings.meeting_id IS NULL")
        }
        Some(crate::types::ListKind::Meeting) => {
            sql.push_str(" AND recordings.meeting_id IS NOT NULL")
        }
        None => {}
    }
    match filter.favorite {
        Some(true) => sql.push_str(" AND recordings.favorite = 1"),
        Some(false) => sql.push_str(" AND recordings.favorite = 0"),
        None => {}
    }
    match filter.pinned {
        Some(true) => sql.push_str(" AND recordings.pinned = 1"),
        Some(false) => sql.push_str(" AND recordings.pinned = 0"),
        None => {}
    }
    match filter.in_place {
        Some(true) => sql.push_str(" AND recordings.in_place = 1"),
        Some(false) => sql.push_str(" AND recordings.in_place = 0"),
        None => {}
    }
    match filter.tagged {
        Some(true) => {
            sql.push_str(" AND recordings.id IN (SELECT recording_id FROM recording_tags)")
        }
        Some(false) => {
            sql.push_str(" AND recordings.id NOT IN (SELECT recording_id FROM recording_tags)")
        }
        None => {}
    }
    if filter.entity_value.is_some() {
        if filter.entity_kind.is_some() {
            sql.push_str(
                " AND recordings.id IN (SELECT recording_id FROM entities WHERE value = ? AND kind = ?)",
            );
        } else {
            sql.push_str(
                " AND recordings.id IN (SELECT recording_id FROM entities WHERE value = ?)",
            );
        }
    }
    match filter.task_state.as_deref() {
        Some("has_open") => {
            sql.push_str(" AND recordings.id IN (SELECT recording_id FROM tasks WHERE done = 0)")
        }
        Some("has_tasks") => {
            sql.push_str(" AND recordings.id IN (SELECT recording_id FROM tasks)")
        }
        _ => {}
    }
    if filter.low_confidence_below.is_some() {
        sql.push_str(
            " AND recordings.mean_confidence IS NOT NULL AND recordings.mean_confidence < ?",
        );
    }
    if filter.since.is_some() {
        sql.push_str(" AND recordings.started_at >= ?");
    }
    if filter.until.is_some() {
        sql.push_str(" AND recordings.started_at <= ?");
    }
    let dir = if filter.sort_desc.unwrap_or(true) {
        "DESC"
    } else {
        "ASC"
    };
    sql.push_str(&format!(
        " ORDER BY recordings.pinned DESC, recordings.started_at {dir}, recordings.id {dir}"
    ));
    match (filter.limit, filter.offset) {
        (Some(n), Some(m)) => sql.push_str(&format!(" LIMIT {n} OFFSET {m}")),
        (Some(n), None) => sql.push_str(&format!(" LIMIT {n}")),
        (None, Some(m)) => sql.push_str(&format!(" LIMIT -1 OFFSET {m}")),
        (None, None) => {}
    }

    // Binds in WHERE order — must mirror the clause order built above exactly.
    let mut binds: Vec<ListBind> = Vec::new();
    if let Some(fq) = fts_query {
        binds.push(ListBind::Text(fq));
    }
    if let Some(tq) = tag_search_query {
        binds.push(ListBind::Text(tq));
    }
    if let Some(mq) = model_search_query {
        // One bind per model column in the WHERE OR above (transcription + cleanup
        // + summary + title + tag + diarization), in that order.
        for _ in 0..6 {
            binds.push(ListBind::Text(mq.clone()));
        }
    }
    if let Some(s) = filter.status {
        binds.push(ListBind::Text(s.as_str().to_string()));
    }
    if let Some(value) = &filter.entity_value {
        binds.push(ListBind::Text(value.clone()));
        if let Some(kind) = &filter.entity_kind {
            binds.push(ListBind::Text(kind.clone()));
        }
    }
    if let Some(thresh) = filter.low_confidence_below {
        binds.push(ListBind::F32(thresh));
    }
    if let Some(t) = filter.since {
        binds.push(ListBind::Text(t.to_rfc3339()));
    }
    if let Some(t) = filter.until {
        binds.push(ListBind::Text(t.to_rfc3339()));
    }

    (sql, binds)
}

impl Catalog {
    /// Insert a new recording row. The pipeline calls this once, when capture
    /// starts; later stages update the same row in place.
    pub async fn insert(&self, r: &Recording) -> Result<()> {
        sqlx::query(
            "INSERT INTO recordings (
                 id, started_at, duration_ms, audio_path, transcript, model, status,
                 error_kind, error_message, hook_command, hook_exit_code, hook_duration_ms,
                 transcribed_at, hook_ran_at, notes, meeting_id, meeting_name, track, in_place,
                 cleanup_model, diarized, ext_ref
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(r.transcribed_at.map(|d| d.to_rfc3339()))
        .bind(r.hook_ran_at.map(|d| d.to_rfc3339()))
        .bind(r.notes.as_deref())
        .bind(r.meeting_id.as_deref())
        .bind(r.meeting_name.as_deref())
        .bind(r.track.as_deref())
        .bind(r.in_place)
        .bind(r.cleanup_model.as_deref())
        .bind(r.diarized)
        // `None` for mic/meeting recordings; set only by an `import --ext-ref`.
        .bind(r.ext_ref.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert a recording with every persisted DTO column at once — the faithful
    /// inverse of the library backup export ([`crate::backup`]).
    ///
    /// The pipeline's [`Catalog::insert`] writes only the columns a fresh
    /// recording starts with and fills the rest (title, summary, favorite, the
    /// per-step model names, …) later via dedicated setters as it advances. A
    /// backup restore has all of those values up front and must land them in one
    /// row, so this writes the full column set. Fields the DTO doesn't carry —
    /// `original_transcript` / `clean_transcript`, segments, words, embeddings,
    /// voiceprints — are bounded by what the export captured and are simply not
    /// restored. This is a plain `INSERT`, not an upsert: the restore caller skips
    /// ids that already exist, so a re-import never overwrites a row (idempotent),
    /// and a genuine id clash surfaces as an error rather than silently
    /// clobbering.
    pub async fn insert_restored(&self, r: &Recording) -> Result<()> {
        let tag_suggestions = if r.tag_suggestions.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&r.tag_suggestions)?)
        };
        sqlx::query(
            "INSERT INTO recordings (
                 id, started_at, duration_ms, audio_path, transcript, model, status,
                 error_kind, error_message, hook_command, hook_exit_code, hook_duration_ms,
                 transcribed_at, hook_ran_at, notes, meeting_id, meeting_name, track, in_place,
                 cleanup_model, diarized, user_edited, favorite, pinned, tag_suggestions, summary,
                 summary_model, entities_model, tasks_model, chapters_model, title, title_is_auto, title_model, tag_model,
                 diarization_model, mean_confidence, detected_language, ext_ref
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(r.transcribed_at.map(|d| d.to_rfc3339()))
        .bind(r.hook_ran_at.map(|d| d.to_rfc3339()))
        .bind(r.notes.as_deref())
        .bind(r.meeting_id.as_deref())
        .bind(r.meeting_name.as_deref())
        .bind(r.track.as_deref())
        .bind(r.in_place)
        .bind(r.cleanup_model.as_deref())
        .bind(r.diarized)
        .bind(r.user_edited)
        .bind(r.favorite)
        .bind(r.pinned)
        .bind(tag_suggestions)
        .bind(r.summary.as_deref())
        .bind(r.summary_model.as_deref())
        .bind(r.entities_model.as_deref())
        .bind(r.tasks_model.as_deref())
        .bind(r.chapters_model.as_deref())
        .bind(r.title.as_deref())
        .bind(r.title_is_auto)
        .bind(r.title_model.as_deref())
        .bind(r.tag_model.as_deref())
        .bind(r.diarization_model.as_deref())
        .bind(r.mean_confidence)
        .bind(r.detected_language.as_deref())
        .bind(r.ext_ref.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The id of an existing recording that carries this external-reference key,
    /// if any. Backs idempotent import: an `import --ext-ref <key>` first looks
    /// here and, on a hit, returns the existing recording instead of importing a
    /// duplicate. Keys are matched exactly (no trimming — the caller normalizes).
    /// `None` means no recording has that key yet.
    pub async fn find_id_by_ext_ref(&self, ext_ref: &str) -> Result<Option<RecordingId>> {
        let row = sqlx::query("SELECT id FROM recordings WHERE ext_ref = ? LIMIT 1")
            .bind(ext_ref)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => {
                let id: String = row.try_get("id")?;
                Ok(RecordingId::parse(&id))
            }
            None => Ok(None),
        }
    }

    /// Set (or clear) the detected spoken language for a recording.
    ///
    /// Called by the pipeline after every transcribe/retranscribe, from the
    /// language the provider reported (see [`crate::transcription::Transcription`]).
    /// `Some(code)` stores the BCP-47/ISO-639 code; `None` clears it back to NULL,
    /// which is what a provider/path that surfaces no language (the native path,
    /// the `gpt-4o-transcribe` family, a plain non-verbose response) writes — so a
    /// retranscribe that drops to such a provider correctly un-detects the
    /// recording instead of leaving a stale language. A NULL value shows no badge
    /// and never matches a language route.
    pub async fn set_detected_language(
        &self,
        id: &RecordingId,
        language: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET detected_language = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(language)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set (or clear) the mean per-word ASR confidence aggregate for a recording.
    ///
    /// Called by the pipeline after every transcribe/retranscribe, from the
    /// already-available word list — never a model re-run. `Some(mean)` stores the
    /// computed mean (see [`crate::ConfidenceAggregate`]); `None` clears it back to
    /// NULL, which is what a provider with no per-word confidence (the OpenAI/Groq
    /// cloud transcription endpoints) or an empty transcript writes — so a
    /// retranscribe that drops to such a provider correctly un-flags the recording
    /// instead of leaving a stale aggregate. A NULL aggregate shows no badge and
    /// never matches the low-confidence filter.
    pub async fn update_confidence(
        &self,
        id: &RecordingId,
        mean_confidence: Option<f32>,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET mean_confidence = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(mean_confidence)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set (or clear) the display name for every track sharing `meeting_id`.
    pub async fn update_meeting_name(&self, meeting_id: &str, name: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE recordings SET meeting_name = ?, updated_at = datetime('now') WHERE meeting_id = ?")
            .bind(name)
            .bind(meeting_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Move a recording to a new lifecycle status (the pipeline calls this as it
    /// advances through transcribing → cleaning up → … → done/failed).
    pub async fn update_status(&self, id: &RecordingId, status: RecordingStatus) -> Result<()> {
        sqlx::query("UPDATE recordings SET status = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(status.as_str())
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record why a recording failed, on the row itself.
    ///
    /// `kind` is the short machine label the failed path already uses for the
    /// inbox quarantine (e.g. `"whisper_error"`, `"hook_failed"`) and `message`
    /// is the human-readable reason. Storing them here makes the failure reason
    /// survive a daemon restart: the live failure events and the `failed/`
    /// quarantine JSON are otherwise the only places it lives, and neither is
    /// readable once the app session that saw the event is gone. The status
    /// itself is set separately by [`Self::update_status`]; this only fills the
    /// two error columns.
    pub async fn update_error(&self, id: &RecordingId, kind: &str, message: &str) -> Result<()> {
        sqlx::query(
            "UPDATE recordings SET error_kind = ?, error_message = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(kind)
        .bind(message)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update both status and duration in a single query.
    pub async fn update_status_and_duration(
        &self,
        id: &RecordingId,
        status: RecordingStatus,
        duration_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE recordings SET status = ?, duration_ms = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(duration_ms)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the transcript after machine transcription.
    ///
    /// `transcript` is the text to store as the live transcript; for recordings
    /// with LLM post-processing enabled this is the LLM-cleaned text.
    /// `original_transcript` is always the raw Whisper output, so "View original"
    /// can show the pre-LLM version even when post-processing is active.
    /// Re-transcription overwrites both columns (a fresh baseline) and clears any
    /// stored failure reason (`error_kind`/`error_message`), so a successful retry
    /// of a previously failed recording stops showing the old error.
    pub async fn update_transcript(
        &self,
        id: &RecordingId,
        transcript: &str,
        original_transcript: &str,
        model: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET transcript = ?, original_transcript = ?, clean_transcript = ?, model = ?,
                   user_edited = 0, error_kind = NULL, error_message = NULL,
                   transcribed_at = datetime('now'), updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(transcript)
        .bind(original_transcript)
        // `clean_transcript` snapshots the pipeline output (transcribed + cleaned)
        // so "View unedited transcript" can show it even after the user edits the
        // live transcript. User edits go through `update_user_transcript`, which
        // leaves this column untouched.
        .bind(transcript)
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record which post-processing LLM model ran (if any), whether speaker
    /// diarization was applied, and the diarizer's model when a cloud diarizer
    /// produced it (`None` for the local speakrs diarizer or none at all).
    /// Called by the pipeline after transcription so the list view and the
    /// detail provenance line can surface these.
    pub async fn update_processing_meta(
        &self,
        id: &RecordingId,
        cleanup_model: Option<&str>,
        diarized: bool,
        diarization_model: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET cleanup_model = ?, diarized = ?, diarization_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(cleanup_model)
        .bind(diarized)
        .bind(diarization_model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the LLM model the auto-tagger used for this recording (the detail
    /// provenance line names it). Written once per auto-tag run, independent of
    /// whether the run produced approve/dismiss suggestions or auto-accepted
    /// existing tags — so the step shows even when nothing was left to approve.
    pub async fn set_tag_model(&self, id: &RecordingId, model: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET tag_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store (or replace) the LLM-generated summary for a recording, along with
    /// the model that produced it.
    pub async fn update_summary(
        &self,
        id: &RecordingId,
        summary: &str,
        model: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET summary = ?, summary_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(summary)
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the transcript from a manual user edit, preserving
    /// `original_transcript`/`clean_transcript` so the edit can be reverted.
    /// Sets the `user_edited` flag and leaves `model` alone, so the "Transcript
    /// Model" column keeps showing the transcription model that actually produced
    /// the text (the hand edit shows up in the "Edited" column instead).
    pub async fn update_user_transcript(&self, id: &RecordingId, transcript: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET transcript = ?, user_edited = 1, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(transcript)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Find-and-replace across a recording's stored live transcript (S6).
    ///
    /// ### Semantics
    /// - Literal substring replacement, not regex — the safest default, with no
    ///   accidental metacharacter surprises on a transcript. `find` is matched
    ///   verbatim; `replace` is inserted verbatim.
    /// - Case sensitivity is opt-out via `case_sensitive`. A case-insensitive match
    ///   still substitutes the user's `replace` text exactly; the original casing
    ///   of each matched run is not preserved (documented).
    /// - Returns the count of occurrences replaced.
    ///
    /// ### Scope
    /// Only the live `transcript` is rewritten — the same column hand edits and
    /// `update_user_transcript` touch. The preserved `original_transcript`
    /// (machine output) and `clean_transcript` (pipeline output) are left intact so
    /// the edit stays revertible; the per-segment and per-word layers are re-flowed
    /// by the caller (the daemon) exactly as for a normal transcript edit. The
    /// recording is marked `user_edited`.
    ///
    /// ### No-op safety
    /// An empty `find`, or zero matches, writes nothing and returns 0. A no-match
    /// never rewrites (and so never corrupts) the transcript, and never touches
    /// `updated_at`/`user_edited`.
    ///
    /// Errors: [`crate::Error::NotFound`] when the id is unknown, or when the
    /// recording has no transcript yet (nothing to edit).
    pub async fn find_replace_transcript(
        &self,
        id: &RecordingId,
        find: &str,
        replace: &str,
        case_sensitive: bool,
    ) -> Result<FindReplaceOutcome> {
        // Empty needle: defined as a no-op (replacing "" would otherwise splice
        // `replace` between every char). Resolve the id first so an unknown id
        // still reports NotFound rather than a silent 0.
        let Some(rec) = self.get(id).await? else {
            return Err(crate::error::Error::NotFound { id: id.to_string() });
        };
        let Some(current) = rec.transcript else {
            return Err(crate::error::Error::NotFound {
                id: format!("{id} (no transcript to edit)"),
            });
        };
        if find.is_empty() {
            return Ok(FindReplaceOutcome {
                replaced: 0,
                transcript: current,
            });
        }

        let (count, new_text) = if case_sensitive {
            (
                current.matches(find).count(),
                current.replace(find, replace),
            )
        } else {
            replace_ignore_case(&current, find, replace)
        };

        // No match → no write: never rewrite on zero matches.
        if count == 0 {
            return Ok(FindReplaceOutcome {
                replaced: 0,
                transcript: current,
            });
        }

        self.update_user_transcript(id, &new_text).await?;
        Ok(FindReplaceOutcome {
            replaced: count,
            transcript: new_text,
        })
    }

    /// Library-wide find-and-replace: run the same literal replace as
    /// [`Self::find_replace_transcript`] over **every** recording's live
    /// transcript, in one call.
    ///
    /// Each recording runs the same literal replace as `find_replace_transcript`
    /// (applied inline over a single batched `id, transcript` read, not a re-`get()`
    /// per row), so the per-recording guarantees carry over verbatim: literal (not
    /// regex) substring matching, case-insensitive by default, only the live
    /// `transcript` is rewritten (the preserved original/clean baselines stay,
    /// keeping each edit revertible), and the recording is marked `user_edited`.
    /// The caller (the daemon) re-flows the timing layers and re-embeds for each
    /// changed recording, exactly as for a single-recording edit.
    ///
    /// ### Skip-on-no-match
    /// A recording with zero matches is left completely untouched — no write, no
    /// version churn, no `updated_at`/`user_edited` change — and is omitted from
    /// `changed`. So the returned `recordings_changed`/`changed` cover exactly the
    /// recordings that were actually rewritten, and the caller emits
    /// `transcript_updated` only for those.
    ///
    /// ### No-op safety
    /// An empty `find` is a whole-operation no-op (`find_replace_transcript`
    /// treats an empty needle as a no-op per recording), returning an empty
    /// outcome without writing anything.
    ///
    /// Recordings with no transcript yet are silently skipped (their
    /// per-recording `NotFound` is not an error for the bulk path) — the bulk
    /// operation is best-effort across the corpus, never aborted by one
    /// untranscribed row.
    pub async fn find_replace_transcript_library(
        &self,
        find: &str,
        replace: &str,
        case_sensitive: bool,
    ) -> Result<FindReplaceLibraryOutcome> {
        // Empty needle: a whole-operation no-op, mirroring the per-recording
        // empty-find contract. Bail before listing the corpus.
        if find.is_empty() {
            return Ok(FindReplaceLibraryOutcome::default());
        }

        // Every recording's id + live transcript in one read. We pull the
        // `transcript` column up front rather than re-`get()`ing each row in the
        // loop: `get()` runs `SELECT *` plus four child queries (speaker names,
        // entities, tasks) per recording, none of which a find-replace ever uses —
        // it only reads the one column and writes it back. So this batch select is
        // the find-replace counterpart of the id-only fetch (no whole-corpus
        // hydration), and the per-recording no-transcript / zero-match guards are
        // applied inline below, mirroring `find_replace_transcript`.
        let rows = sqlx::query("SELECT id, transcript FROM recordings")
            .fetch_all(&self.pool)
            .await?;

        let mut outcome = FindReplaceLibraryOutcome::default();
        for row in rows {
            let id = RecordingId::from_string(row.try_get("id")?);
            // No transcript yet → benign skip (the per-recording path reports this
            // as NotFound, which the bulk path treats as a skip, not a failure).
            let Some(current) = row.try_get::<Option<String>, _>("transcript")? else {
                continue;
            };

            let (count, new_text) = if case_sensitive {
                (
                    current.matches(find).count(),
                    current.replace(find, replace),
                )
            } else {
                replace_ignore_case(&current, find, replace)
            };
            // Zero-match: nothing to write; skip it entirely (no event, no churn).
            if count == 0 {
                continue;
            }

            // Write the rewritten transcript directly via the same setter
            // `find_replace_transcript` ends on — marks `user_edited`, leaves the
            // original/clean baselines intact so the edit stays revertible.
            match self.update_user_transcript(&id, &new_text).await {
                Ok(()) => {
                    outcome.recordings_changed += 1;
                    outcome.total_replacements += count;
                    outcome.changed.push((id, new_text));
                }
                // A write failure is logged and skipped so one bad row can't abort
                // the whole sweep — but it's counted as a failure so the caller can
                // surface it rather than silently reporting a smaller success count.
                Err(e) => {
                    tracing::warn!(id = %id, error = %e, "library find-replace: a recording failed to update");
                    outcome.failed += 1;
                    outcome.failed_ids.push(id);
                }
            }
        }
        Ok(outcome)
    }

    /// Replace the LLM-suggested tags awaiting approval for a recording.
    /// An empty slice clears the column (no lingering empty-array JSON).
    pub async fn set_tag_suggestions(&self, id: &RecordingId, names: &[String]) -> Result<()> {
        let json = if names.is_empty() {
            None
        } else {
            Some(serde_json::to_string(names)?)
        };
        sqlx::query(
            r#"UPDATE recordings
               SET tag_suggestions = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(json)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drop every pending tag suggestion across the whole library (the
    /// Auto-Tagging settings' "Clear all suggestions" action). Returns how many
    /// recordings actually had suggestions to clear.
    pub async fn clear_all_tag_suggestions(&self) -> Result<u64> {
        let result = sqlx::query(
            r#"UPDATE recordings
               SET tag_suggestions = NULL, updated_at = datetime('now')
               WHERE tag_suggestions IS NOT NULL"#,
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Set or clear a recording's display title.
    ///
    /// Ownership rule: a user title (`title_is_auto = 0`) wins for good. An auto
    /// write (`is_auto = true` with a title) only lands while the title is still
    /// auto-owned, so the pipeline can refresh its own titles on retranscribe but
    /// can never clobber one the user typed. Explicit user writes always apply:
    /// `Some` with `is_auto = false` takes ownership; `None` clears the title and
    /// reverts ownership to auto, so the next pipeline run generates a fresh one.
    ///
    /// Returns whether a row was actually updated (`false` = unknown id, or
    /// an auto write skipped because the user owns the title).
    ///
    /// `model` records which LLM produced an auto title for the provenance line:
    /// the auto-title step passes `Some(model)` when an LLM made the title and
    /// `None` for a heuristic one; user/CLI title writes pass `None`, which also
    /// clears any stale model so a user-owned title never shows one.
    pub async fn set_title(
        &self,
        id: &RecordingId,
        title: Option<&str>,
        is_auto: bool,
        model: Option<&str>,
    ) -> Result<bool> {
        // A cleared title is always auto-owned — `None` means "no title,
        // generate one next run", never "user-owned empty title".
        let is_auto = is_auto || title.is_none();
        let result = if is_auto && title.is_some() {
            sqlx::query(
                r#"UPDATE recordings
                   SET title = ?, title_is_auto = 1, title_model = ?, updated_at = datetime('now')
                   WHERE id = ? AND title_is_auto = 1"#,
            )
            .bind(title)
            .bind(model)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"UPDATE recordings
                   SET title = ?, title_is_auto = ?, title_model = ?, updated_at = datetime('now')
                   WHERE id = ?"#,
            )
            .bind(title)
            .bind(is_auto)
            .bind(model)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?
        };
        Ok(result.rows_affected() > 0)
    }

    /// Set or clear the "favorite"/star flag for a recording (Favorites view).
    pub async fn set_favorite(&self, id: &RecordingId, favorite: bool) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET favorite = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(favorite as i64)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set or clear the "pinned" flag for a recording (Pinned view). Pinned
    /// recordings sort to the top of the library, independent of `favorite`.
    pub async fn set_pinned(&self, id: &RecordingId, pinned: bool) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET pinned = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(pinned as i64)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The preserved original (machine) transcript, if any. `None` for
    /// recordings transcribed before this column existed, or never transcribed.
    pub async fn get_original_transcript(&self, id: &RecordingId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT original_transcript FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(r.try_get::<Option<String>, _>("original_transcript")?),
            None => Ok(None),
        }
    }

    /// The preserved "unedited" transcript — the pipeline output (machine
    /// transcription + any LLM cleanup) before the user made hand edits. `None`
    /// for recordings transcribed before this column existed, or never
    /// transcribed.
    pub async fn get_clean_transcript(&self, id: &RecordingId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT clean_transcript FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(r.try_get::<Option<String>, _>("clean_transcript")?),
            None => Ok(None),
        }
    }

    /// Update the free-form user notes for a recording.
    ///
    /// Notes live in their own column and are completely independent of the
    /// transcript: neither machine (re-)transcription (`update_transcript`)
    /// nor user transcript edits (`update_user_transcript`) touch this column,
    /// so notes always survive those operations.
    pub async fn update_notes(&self, id: &RecordingId, notes: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET notes = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(notes)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the outcome of the hook that ran for a recording (command, exit
    /// code, duration), stamping `hook_ran_at`.
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

    /// The recording's Meeting-Mode track and meeting link — `(track,
    /// meeting_id)` — without the speaker-name join [`get`](Self::get) does.
    ///
    /// The pipeline needs only these two columns before transcribing, to drive
    /// track-aware Meeting Mode (a meeting's mic track is labelled as one fixed
    /// speaker instead of diarized). This narrow read keeps that hot path from
    /// paying for the full row and its join. Both columns are `None` for a normal
    /// single-track recording; `(None, None)` when the id is unknown.
    pub async fn track_and_meeting(
        &self,
        id: &RecordingId,
    ) -> Result<(Option<String>, Option<String>)> {
        let row = sqlx::query("SELECT track, meeting_id FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok((r.try_get("track")?, r.try_get("meeting_id")?)),
            None => Ok((None, None)),
        }
    }

    /// Fetch a single recording by id, with its custom speaker names populated
    /// (tags are loaded separately via [`Catalog::tags_for`]). `None` when the
    /// id is unknown.
    pub async fn get(&self, id: &RecordingId) -> Result<Option<Recording>> {
        let row = sqlx::query("SELECT * FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        let mut rec = match row.map(row_to_recording).transpose()? {
            Some(r) => r,
            None => return Ok(None),
        };
        // Populate the speaker-name map so a single-recording fetch (the daemon's
        // GetRecording, which backs the detail view) can render custom names.
        // Tags are deliberately left out here — the detail view loads those
        // separately via `tags_for`.
        rec.speaker_names = self.speaker_names_for(&rec.id).await.unwrap_or_default();
        // Entities populate here (unlike tags): the detail view's entity surface
        // reads `Recording::entities` straight off GetRecording, with no separate
        // fetch of its own. Best-effort — a child-query failure leaves it empty.
        rec.entities = self.list_entities(&rec.id).await.unwrap_or_default();
        // Tasks populate here too (like entities): the detail pane's task chips
        // read `Recording::tasks` straight off GetRecording. Best-effort.
        rec.tasks = self.list_tasks(&rec.id).await.unwrap_or_default();
        Ok(Some(rec))
    }

    /// Bulk counterpart of [`Self::get`]: fetch many recordings by id in one
    /// `WHERE id IN (…)` query (chunked under SQLite's bound-param cap) plus three
    /// batched child queries, instead of a full round-trip per id. Populates the
    /// same children as `get` (speaker names + entities + tasks, NOT tags). Order
    /// is unspecified — callers that need a particular order (e.g. by search score)
    /// re-join on id. Ids absent from the catalog are simply omitted. Used by the
    /// semantic-search / more-like-this handlers, which otherwise issued up to
    /// `MAX_SEARCH_RESULTS` sequential `get` calls.
    pub async fn get_batch(&self, ids: &[RecordingId]) -> Result<Vec<Recording>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let id_strs: Vec<String> = ids.iter().map(|i| i.as_str().to_string()).collect();
        let mut recs: Vec<Recording> = Vec::new();
        for chunk in id_strs.chunks(IN_CHUNK) {
            let ph = in_placeholders(chunk.len());
            let sql = format!("SELECT * FROM recordings WHERE id IN ({ph})");
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            for row in q.fetch_all(&self.pool).await? {
                recs.push(row_to_recording(row)?);
            }
        }
        // Match `get`'s children — entities + tasks + speaker names (tags are
        // deliberately left out, callers fetch those separately) — but batched
        // into one query each over the whole set rather than per recording.
        let ids2: Vec<String> = recs.iter().map(|r| r.id.as_str().to_string()).collect();
        let mut entities = self.entities_for_many(&ids2).await.unwrap_or_default();
        let mut tasks = self.tasks_for_many(&ids2).await.unwrap_or_default();
        let mut speaker_names = self.speaker_names_for_many(&ids2).await.unwrap_or_default();
        for rec in &mut recs {
            let key = rec.id.as_str().to_string();
            rec.entities = entities.remove(&key).unwrap_or_default();
            rec.tasks = tasks.remove(&key).unwrap_or_default();
            rec.speaker_names = speaker_names.remove(&key).unwrap_or_default();
        }
        Ok(recs)
    }

    /// List recordings matching `filter`, pinned-first then newest-first by
    /// default, with tags and speaker names populated per row.
    ///
    /// Every predicate — full-text search, tag, status, kind (single vs. meeting),
    /// favorites, pinned, date range — is applied in SQL before `LIMIT`/`OFFSET`,
    /// so pagination composes correctly. Filtering after pagination would return
    /// mostly-empty pages of the chosen kind. Pinned recordings always sort to the
    /// top (`pinned DESC` leads the ORDER BY), ahead of the date sort. Backs the
    /// GUI Library and the CLI `phoneme list`.
    pub async fn list(&self, filter: &ListFilter) -> Result<Vec<Recording>> {
        let (suffix, binds) = list_query_suffix(filter);
        let sql = format!("SELECT recordings.* FROM recordings{suffix}");
        let mut q = sqlx::query(&sql);
        for b in &binds {
            q = match b {
                ListBind::Text(s) => q.bind(s.clone()),
                ListBind::F32(f) => q.bind(*f),
            };
        }
        let rows = q.fetch_all(&self.pool).await?;
        let mut recs: Vec<Recording> = rows
            .into_iter()
            .map(row_to_recording)
            .collect::<Result<_>>()?;
        // Populate tags, entities, tasks, and custom speaker names per row. Each
        // child table is read in ONE batched `WHERE recording_id IN (…)` query over
        // the whole page and bucketed back per recording, rather than four queries
        // per row (the old N+1). Best-effort, matching the per-row `.unwrap_or_default()`
        // this replaced: a child-query failure leaves that child empty rather than
        // failing the list.
        let ids: Vec<String> = recs.iter().map(|r| r.id.as_str().to_string()).collect();
        let mut tags = self.tags_for_many(&ids).await.unwrap_or_default();
        let mut entities = self.entities_for_many(&ids).await.unwrap_or_default();
        let mut tasks = self.tasks_for_many(&ids).await.unwrap_or_default();
        let mut speaker_names = self.speaker_names_for_many(&ids).await.unwrap_or_default();
        for rec in &mut recs {
            let key = rec.id.as_str().to_string();
            rec.tags = tags.remove(&key).unwrap_or_default();
            rec.entities = entities.remove(&key).unwrap_or_default();
            rec.tasks = tasks.remove(&key).unwrap_or_default();
            rec.speaker_names = speaker_names.remove(&key).unwrap_or_default();
        }
        Ok(recs)
    }

    /// Just the `(id, meeting_id)` pairs matching a filter — no row
    /// deserialization, no tags/entities/tasks/speaker child queries. Shares the
    /// exact filter SQL with [`Self::list`] via [`list_query_suffix`]. `fuse_hybrid`
    /// uses this to build the in-scope key set for a filtered semantic search,
    /// where the full `list()` would fetch then immediately discard every
    /// `Recording` and all four of its child collections.
    pub async fn list_ids(&self, filter: &ListFilter) -> Result<Vec<(String, Option<String>)>> {
        let (suffix, binds) = list_query_suffix(filter);
        let sql = format!("SELECT recordings.id, recordings.meeting_id FROM recordings{suffix}");
        let mut q = sqlx::query(&sql);
        for b in &binds {
            q = match b {
                ListBind::Text(s) => q.bind(s.clone()),
                ListBind::F32(f) => q.bind(*f),
            };
        }
        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                let id: String = row.try_get("id")?;
                let meeting_id: Option<String> = row.try_get("meeting_id")?;
                Ok((id, meeting_id))
            })
            .collect()
    }

    /// Batched counterpart of [`Self::tags_for`]: every page recording's tags in
    /// one query, keyed by recording id. Per-recording order matches `tags_for`
    /// (name-sorted). The id list is chunked under SQLite's bound-parameter cap so
    /// an unpaginated `list()` over a large corpus stays one query per chunk.
    async fn tags_for_many(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<Tag>>> {
        let mut map: std::collections::HashMap<String, Vec<Tag>> = std::collections::HashMap::new();
        for chunk in ids.chunks(IN_CHUNK) {
            let sql = format!(
                "SELECT rt.recording_id, t.id, t.name, t.color \
                 FROM tags t JOIN recording_tags rt ON rt.tag_id = t.id \
                 WHERE rt.recording_id IN ({}) \
                 ORDER BY t.name",
                in_placeholders(chunk.len())
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for r in rows {
                let rid: String = r.try_get("recording_id")?;
                map.entry(rid).or_default().push(Tag {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    color: r.try_get("color")?,
                });
            }
        }
        Ok(map)
    }

    /// Batched counterpart of [`Self::list_entities`]: every page recording's
    /// entities in one query, keyed by recording id. Per-recording order matches
    /// `list_entities` (kind, then value).
    async fn entities_for_many(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<Entity>>> {
        let mut map: std::collections::HashMap<String, Vec<Entity>> =
            std::collections::HashMap::new();
        for chunk in ids.chunks(IN_CHUNK) {
            let sql = format!(
                "SELECT recording_id, kind, value FROM entities \
                 WHERE recording_id IN ({}) \
                 ORDER BY kind, value",
                in_placeholders(chunk.len())
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for r in rows {
                let rid: String = r.try_get("recording_id")?;
                map.entry(rid).or_default().push(Entity {
                    kind: r.try_get("kind")?,
                    value: r.try_get("value")?,
                });
            }
        }
        Ok(map)
    }

    /// Batched counterpart of [`Self::list_tasks`]: every page recording's tasks in
    /// one query, keyed by recording id. Per-recording order matches `list_tasks`
    /// (open first, then sort_order, then row id).
    async fn tasks_for_many(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<Task>>> {
        let mut map: std::collections::HashMap<String, Vec<Task>> =
            std::collections::HashMap::new();
        for chunk in ids.chunks(IN_CHUNK) {
            let sql = format!(
                "SELECT recording_id, id, text, due_hint, done FROM tasks \
                 WHERE recording_id IN ({}) \
                 ORDER BY recording_id, done, sort_order, id",
                in_placeholders(chunk.len())
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for r in rows {
                let rid: String = r.try_get("recording_id")?;
                map.entry(rid).or_default().push(Task {
                    id: r.try_get("id")?,
                    text: r.try_get("text")?,
                    due_hint: r.try_get("due_hint")?,
                    done: r.try_get("done")?,
                });
            }
        }
        Ok(map)
    }

    /// Batched counterpart of [`Self::speaker_names_for`]: every page recording's
    /// custom speaker names in one query, keyed by recording id. Per-recording
    /// order matches `speaker_names_for` (by speaker label).
    async fn speaker_names_for_many(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<SpeakerName>>> {
        let mut map: std::collections::HashMap<String, Vec<SpeakerName>> =
            std::collections::HashMap::new();
        for chunk in ids.chunks(IN_CHUNK) {
            let sql = format!(
                "SELECT recording_id, speaker_label, name FROM speaker_names \
                 WHERE recording_id IN ({}) \
                 ORDER BY recording_id, speaker_label",
                in_placeholders(chunk.len())
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for r in rows {
                let rid: String = r.try_get("recording_id")?;
                map.entry(rid).or_default().push(SpeakerName {
                    speaker_label: r.try_get("speaker_label")?,
                    name: r.try_get("name")?,
                });
            }
        }
        Ok(map)
    }

    /// Fetch all recordings belonging to a single meeting session.
    ///
    /// Returns the rows that share `meeting_id`, ordered by `track` then
    /// `started_at` so the two tracks of a meeting come back in a stable order
    /// (e.g. "mic" before "system", since "mic" < "system" lexicographically).
    /// A `meeting_id` with no rows yields an empty `Vec` (not an error) — the
    /// caller treats that as "no such session".
    pub async fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<Recording>> {
        let rows = sqlx::query(
            "SELECT * FROM recordings WHERE meeting_id = ? \
             ORDER BY track ASC, started_at ASC, id ASC",
        )
        .bind(meeting_id)
        .fetch_all(&self.pool)
        .await?;
        let mut recs: Vec<Recording> = rows
            .into_iter()
            .map(row_to_recording)
            .collect::<Result<_>>()?;
        // The merged meeting view maps `[Speaker N]` → custom names per track, so
        // each track must carry its own speaker-name map.
        for rec in &mut recs {
            rec.speaker_names = self.speaker_names_for(&rec.id).await.unwrap_or_default();
        }
        Ok(recs)
    }

    /// Delete a recording's catalog row. Cascading foreign keys take its tags,
    /// segments, speaker names, and embeddings with it; the caller removes the
    /// audio file from disk separately.
    pub async fn delete(&self, id: &RecordingId) -> Result<()> {
        // Named voices that will lose a sample when the cascade removes this
        // recording's voiceprints. Capture them before the delete so we can
        // recompute their cached centroids afterward (audit H1). Null links are
        // skipped.
        let affected: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT named_voice_id FROM speaker_voiceprints \
             WHERE recording_id = ? AND named_voice_id IS NOT NULL",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;

        // Capture the recording's ANN keys before the DELETE: the FK cascade
        // removes its `ann_keys` rows, so afterwards we couldn't find them to
        // drop the matching nodes from the in-memory index. A no-op unless ANN
        // is enabled.
        let ann_keys = self.recording_ann_keys_for_delete(id).await;

        sqlx::query("DELETE FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        // The cascade took this recording's embeddings, voiceprints, dismissed
        // suggestions, and ann_keys with it — patch its now-empty vectors out of
        // the warm cache, drop its nodes from the ANN index, then recompute any
        // named voice that just lost a sample so its centroid and count stay
        // accurate.
        self.patch_recording_in_cache(id).await;
        self.remove_recording_from_ann_keys(&ann_keys).await;
        for nid in affected {
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(())
    }

    /// Delete every recording row — and, via the same cascade as [`Self::delete`],
    /// all their tags, segments, words, speaker names, and embeddings. Used by the
    /// destructive catalog rebuild, which then re-imports the audio from disk.
    /// Returns the number of rows removed. The caller leaves the WAV files on disk
    /// (the rebuild re-links them).
    pub async fn clear_all_recordings(&self) -> Result<u64> {
        // Named voices that will lose samples when the cascade removes every
        // recording's voiceprints. Capture them before the delete so their cached
        // centroids and counts can be recomputed afterward, mirroring
        // [`Self::delete`] (audit M1). Null links are skipped.
        let affected: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT named_voice_id FROM speaker_voiceprints \
             WHERE named_voice_id IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        let res = sqlx::query("DELETE FROM recordings")
            .execute(&self.pool)
            .await?;
        self.invalidate_embedding_cache();
        // Every recording's ann_keys went with the cascade — drop the in-memory
        // index and its sidecar so search falls back to brute force until a
        // re-import re-embeds and the daemon rebuilds (no-op without the feature).
        self.clear_ann_index();
        for nid in affected {
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(res.rows_affected())
    }

    /// Run an explicit WAL checkpoint. PASSIVE mode is non-blocking — readers
    /// can keep going while the checkpoint runs. Day-to-day the `-wal` file is
    /// bounded by the `wal_autocheckpoint=1000` pragma set at open (see
    /// [`Catalog::open`]), which checkpoints automatically as the WAL grows; this
    /// is an explicit on-demand checkpoint for callers that want to force one.
    pub async fn checkpoint(&self) -> Result<()> {
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
