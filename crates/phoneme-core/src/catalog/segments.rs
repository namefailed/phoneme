//! Transcript segments, versions, words, and speaker-edit operations.

use super::*;

impl Catalog {
    /// Replace a recording's machine transcript segments with a fresh set.
    ///
    /// Called by the pipeline after every transcribe/retranscribe. Segments always
    /// describe the current machine output, so the old rows are dropped first, in
    /// the same transaction, so a crash can't leave a half-replaced timeline. An
    /// empty slice just clears them (e.g. a provider that returns no timing data).
    pub async fn replace_segments(
        &self,
        recording_id: &RecordingId,
        segments: &[TranscriptSegment],
    ) -> Result<()> {
        self.replace_segments_variant(recording_id, "raw", segments)
            .await
    }

    /// Replace one timing variant of a recording's segments, leaving the other
    /// variant intact (TL-CONSISTENCY). `"raw"` is the machine-truth timeline (what
    /// [`replace_segments`](Self::replace_segments) writes); `"cleaned"` is the
    /// timeline re-aligned to the post-cleanup transcript. Same
    /// replace-on-(re)transcribe semantics, scoped to `variant`.
    ///
    /// Caveat: the U1 speaker-correction ops (`reassign_segment`/merge/split) edit
    /// `transcript_segments` by `(recording_id, idx)` without a variant filter.
    /// That's harmless before any `"cleaned"` rows exist (only `"raw"` rows are
    /// present), but those ops need to be scoped to `"raw"` once the cleaned
    /// re-flow is wired.
    pub async fn replace_segments_variant(
        &self,
        recording_id: &RecordingId,
        variant: &str,
        segments: &[TranscriptSegment],
    ) -> Result<()> {
        let table = segments_table(variant);
        let mut tx = self.pool.begin().await?;
        sqlx::query(&format!("DELETE FROM {table} WHERE recording_id = ?"))
            .bind(recording_id.as_str())
            .execute(&mut *tx)
            .await?;
        for (idx, seg) in segments.iter().enumerate() {
            sqlx::query(&format!(
                "INSERT INTO {table} (recording_id, idx, start_ms, end_ms, text, speaker) \
                 VALUES (?, ?, ?, ?, ?, ?)"
            ))
            .bind(recording_id.as_str())
            .bind(idx as i64)
            .bind(seg.start_ms)
            .bind(seg.end_ms)
            .bind(&seg.text)
            .bind(&seg.speaker)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A recording's machine transcript segments in timeline order. Empty when
    /// the recording predates segment capture or its provider returned no
    /// timing data — callers must treat "no segments" as a normal state, not
    /// an error.
    pub async fn segments_for(&self, recording_id: &RecordingId) -> Result<Vec<TranscriptSegment>> {
        self.segments_for_variant(recording_id, "raw").await
    }

    /// A recording's segments for one timing `variant` (`"raw"` or `"cleaned"`),
    /// in timeline order. Empty when that variant has no rows (a recording with no
    /// cleanup has no `"cleaned"` timeline) — a normal state, not an error.
    pub async fn segments_for_variant(
        &self,
        recording_id: &RecordingId,
        variant: &str,
    ) -> Result<Vec<TranscriptSegment>> {
        let table = segments_table(variant);
        let rows = sqlx::query(&format!(
            "SELECT start_ms, end_ms, text, speaker FROM {table} \
             WHERE recording_id = ? ORDER BY idx"
        ))
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(TranscriptSegment {
                    start_ms: r.try_get("start_ms")?,
                    end_ms: r.try_get("end_ms")?,
                    text: r.try_get("text")?,
                    speaker: r.try_get("speaker")?,
                })
            })
            .collect()
    }

    // ── Compounding transcript versions (PB-COMPOUND) ───────────────────────
    //
    // A compounding recipe chains Transform steps (each rewrites the running
    // transcript); these methods record every step's output so the chain is
    // inspectable + revertible. Replaced wholesale per (re)transcription, like
    // segments. The executor + the Compare-versions IPC/UI wire onto these.

    /// Replace all transcript versions for a recording (wholesale, single tx).
    /// Pass them in `idx` order (`0` = raw ASR). An empty slice clears them.
    pub async fn replace_transcript_versions(
        &self,
        recording_id: &RecordingId,
        versions: &[TranscriptVersion],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM transcript_versions WHERE recording_id = ?")
            .bind(recording_id.as_str())
            .execute(&mut *tx)
            .await?;
        for v in versions {
            sqlx::query(
                "INSERT INTO transcript_versions \
                 (recording_id, idx, step_id, label, model, text) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(recording_id.as_str())
            .bind(v.idx)
            .bind(&v.step_id)
            .bind(&v.label)
            .bind(&v.model)
            .bind(&v.text)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A recording's transcript versions in step order. Empty for recordings that
    /// predate compounding — callers treat "no versions" as a normal state.
    pub async fn transcript_versions_for(
        &self,
        recording_id: &RecordingId,
    ) -> Result<Vec<TranscriptVersion>> {
        let rows = sqlx::query(
            "SELECT idx, step_id, label, model, text FROM transcript_versions \
             WHERE recording_id = ? ORDER BY idx",
        )
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(TranscriptVersion {
                    idx: r.try_get("idx")?,
                    step_id: r.try_get("step_id")?,
                    label: r.try_get("label")?,
                    model: r.try_get("model")?,
                    text: r.try_get("text")?,
                })
            })
            .collect()
    }

    /// One transcript version by step `idx`, if present.
    pub async fn transcript_version(
        &self,
        recording_id: &RecordingId,
        idx: i64,
    ) -> Result<Option<TranscriptVersion>> {
        let row = sqlx::query(
            "SELECT idx, step_id, label, model, text FROM transcript_versions \
             WHERE recording_id = ? AND idx = ?",
        )
        .bind(recording_id.as_str())
        .bind(idx)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(TranscriptVersion {
                idx: r.try_get("idx")?,
                step_id: r.try_get("step_id")?,
                label: r.try_get("label")?,
                model: r.try_get("model")?,
                text: r.try_get("text")?,
            })
        })
        .transpose()
    }

    // ── In-recording speaker correction (U1) ───────────────────────────────
    //
    // Let the user fix the diarizer's per-segment speaker assignments after the
    // fact: reassign one segment, merge two speakers into one, or split some
    // segments off into a fresh speaker. `transcript_segments.speaker` is the
    // authoritative store — the timeline and Synced views re-derive from it. The
    // prose `transcript` text's `[Speaker N]:` markers are a separate display
    // source (the detail prose view and the rename modal read them), so every op
    // that changes which segment belongs to which speaker also rebuilds those
    // markers from the updated segments in the same transaction; otherwise the
    // prose view would disagree with the timeline. Labels are the 1-based integer
    // `[Speaker N]` index that also keys `speaker_names` / `speaker_voiceprints`;
    // in `transcript_segments`/`transcript_words` they're stored as that integer's
    // text form ("1", "2", …).

    /// Reassign one segment to a different speaker label.
    ///
    /// Sets `transcript_segments[idx].speaker` to `new_label` (and the matching
    /// `transcript_words` rows, so the word layer agrees), then rebuilds the
    /// prose transcript's `[Speaker N]:` markers from the updated segments. A
    /// brand-new `new_label` simply starts existing — no name or voiceprint is
    /// created for it. Errors with [`Error::NotFound`](crate::error::Error) if no
    /// segment has that `idx` (no write happens). `new_label` must be ≥ 1.
    pub async fn reassign_segment(
        &self,
        recording_id: &RecordingId,
        idx: i64,
        new_label: i64,
    ) -> Result<()> {
        if new_label < 1 {
            return Err(crate::error::Error::Internal(format!(
                "invalid speaker label {new_label} (must be >= 1)"
            )));
        }
        let mut tx = self.pool.begin().await?;
        let label_text = new_label.to_string();
        // Match the segment by idx; capture its span so the word layer can be
        // repointed to the same speaker over the same time window.
        let span = sqlx::query(
            "SELECT start_ms, end_ms FROM transcript_segments WHERE recording_id = ? AND idx = ?",
        )
        .bind(recording_id.as_str())
        .bind(idx)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(span) = span else {
            return Err(crate::error::Error::NotFound {
                id: format!("segment {idx} of recording {}", recording_id.as_str()),
            });
        };
        let (start_ms, end_ms): (i64, i64) = (span.try_get("start_ms")?, span.try_get("end_ms")?);
        sqlx::query(
            "UPDATE transcript_segments SET speaker = ? WHERE recording_id = ? AND idx = ?",
        )
        .bind(&label_text)
        .bind(recording_id.as_str())
        .bind(idx)
        .execute(&mut *tx)
        .await?;
        // Keep the per-word layer in step: words inside the segment's span get the
        // same new label. Words carry no idx tie to a segment, so the time span is
        // the join — the diarizer builds both from one attribution, so a word's
        // span lies within its segment's.
        sqlx::query(
            "UPDATE transcript_words SET speaker = ? \
             WHERE recording_id = ? AND start_ms >= ? AND start_ms < ?",
        )
        .bind(&label_text)
        .bind(recording_id.as_str())
        .bind(start_ms)
        .bind(end_ms)
        .execute(&mut *tx)
        .await?;
        Self::rebuild_transcript_markers_tx(&mut tx, recording_id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Merge `from_label` into `into_label`: every segment (and word) of `from`
    /// becomes `into`, the `from` label ceases to exist.
    ///
    /// Consistency, all in one transaction:
    /// - Segments and words: repoint `speaker = from` → `into`.
    /// - Names: keep `into`'s name; if `into` has none, adopt `from`'s. Either way
    ///   the now-defunct `from` name row is deleted.
    /// - Voiceprints: the captured centroid is per recording-label, so a merged
    ///   speaker's two captures can't be averaged into one meaningful centroid here
    ///   (that would need the raw frames). The simplest correct choice is to drop
    ///   `from`'s capture row — and recompute its formerly-linked named voice so the
    ///   library no longer counts it — while `into` keeps its own capture. A
    ///   re-transcribe re-captures a fresh, correct centroid for the merged label.
    ///   This favours a clean library over a centroid blended from the wrong inputs.
    ///
    /// Errors with [`Error::NotFound`](crate::error::Error) when no segment carries
    /// `from_label` (nothing to merge, so no write). `from`/`into` must be ≥ 1 and
    /// differ.
    pub async fn merge_speakers(
        &self,
        recording_id: &RecordingId,
        from_label: i64,
        into_label: i64,
    ) -> Result<()> {
        if from_label < 1 || into_label < 1 {
            return Err(crate::error::Error::Internal(format!(
                "invalid speaker labels (from={from_label}, into={into_label}; must be >= 1)"
            )));
        }
        if from_label == into_label {
            return Err(crate::error::Error::Internal(
                "cannot merge a speaker into itself".into(),
            ));
        }
        let rid = recording_id.as_str();
        let from_text = from_label.to_string();
        let into_text = into_label.to_string();

        // The named voice that `from`'s capture was enrolled under (if any) needs
        // recomputing once its row is gone, so it stops counting the dropped
        // sample. Read it before the transaction's writes.
        let from_named_voice = self.named_voice_for(rid, from_label).await?;

        let mut tx = self.pool.begin().await?;
        // Guard: `from` must actually appear, else this is a no-op the caller
        // should hear about as an error (not a silent success).
        let from_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM transcript_segments WHERE recording_id = ? AND speaker = ?",
        )
        .bind(rid)
        .bind(&from_text)
        .fetch_one(&mut *tx)
        .await?;
        if from_count == 0 {
            return Err(crate::error::Error::NotFound {
                id: format!("speaker {from_label} of recording {rid}"),
            });
        }
        // Segments + words: repoint from → into.
        sqlx::query(
            "UPDATE transcript_segments SET speaker = ? WHERE recording_id = ? AND speaker = ?",
        )
        .bind(&into_text)
        .bind(rid)
        .bind(&from_text)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE transcript_words SET speaker = ? WHERE recording_id = ? AND speaker = ?",
        )
        .bind(&into_text)
        .bind(rid)
        .bind(&from_text)
        .execute(&mut *tx)
        .await?;

        // Names: `into` keeps its own; adopt `from`'s only when `into` is unnamed.
        let into_named: Option<String> = sqlx::query_scalar(
            "SELECT name FROM speaker_names WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(rid)
        .bind(into_label)
        .fetch_optional(&mut *tx)
        .await?;
        let from_named: Option<String> = sqlx::query_scalar(
            "SELECT name FROM speaker_names WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(rid)
        .bind(from_label)
        .fetch_optional(&mut *tx)
        .await?;
        if into_named.is_none() {
            if let Some(name) = from_named {
                sqlx::query(
                    "INSERT INTO speaker_names (recording_id, speaker_label, name) \
                     VALUES (?, ?, ?) \
                     ON CONFLICT(recording_id, speaker_label) DO UPDATE SET name = excluded.name",
                )
                .bind(rid)
                .bind(into_label)
                .bind(&name)
                .execute(&mut *tx)
                .await?;
            }
        }
        // The `from` name row is now defunct — drop it.
        sqlx::query("DELETE FROM speaker_names WHERE recording_id = ? AND speaker_label = ?")
            .bind(rid)
            .bind(from_label)
            .execute(&mut *tx)
            .await?;

        // Voiceprints: drop `from`'s capture (documented choice — see doc above).
        // `into`'s capture is left untouched.
        sqlx::query("DELETE FROM speaker_voiceprints WHERE recording_id = ? AND speaker_label = ?")
            .bind(rid)
            .bind(from_label)
            .execute(&mut *tx)
            .await?;
        // Any dismissed-suggestion row for the now-gone label is dead weight.
        sqlx::query(
            "DELETE FROM dismissed_speaker_suggestions WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(rid)
        .bind(from_label)
        .execute(&mut *tx)
        .await?;

        Self::rebuild_transcript_markers_tx(&mut tx, recording_id).await?;
        tx.commit().await?;

        // Recompute the library entry `from` fed, now that its sample is gone, so
        // the cross-recording centroid/count no longer reflect the dropped row.
        if let Some(nid) = from_named_voice {
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(())
    }

    /// Split: move `segment_idxs` from `label` onto a fresh `new_label`.
    ///
    /// The listed segments (and their words) become `new_label`; every other
    /// segment of `label` stays put. The new label has no name and no voiceprint
    /// until the user names or re-enrolls it. The prose markers are rebuilt from
    /// the updated segments. Errors with [`Error::NotFound`](crate::error::Error)
    /// if any listed idx is missing or doesn't currently carry `label` (no partial
    /// write). `label`/`new_label` must be ≥ 1 and differ, and the idx list must be
    /// non-empty.
    pub async fn split_speaker(
        &self,
        recording_id: &RecordingId,
        label: i64,
        segment_idxs: &[i64],
        new_label: i64,
    ) -> Result<()> {
        if label < 1 || new_label < 1 {
            return Err(crate::error::Error::Internal(format!(
                "invalid speaker labels (label={label}, new={new_label}; must be >= 1)"
            )));
        }
        if label == new_label {
            return Err(crate::error::Error::Internal(
                "split target label must differ from the source".into(),
            ));
        }
        if segment_idxs.is_empty() {
            return Err(crate::error::Error::Internal(
                "split requires at least one segment index".into(),
            ));
        }
        let rid = recording_id.as_str();
        let label_text = label.to_string();
        let new_text = new_label.to_string();

        let mut tx = self.pool.begin().await?;
        // Validate every idx first — each must exist and currently belong to
        // `label` — collecting spans so the word layer can be repointed. A single
        // bad idx aborts the whole op with no write (the transaction is rolled
        // back).
        let mut spans: Vec<(i64, i64)> = Vec::with_capacity(segment_idxs.len());
        for &idx in segment_idxs {
            let row = sqlx::query(
                "SELECT start_ms, end_ms, speaker FROM transcript_segments \
                 WHERE recording_id = ? AND idx = ?",
            )
            .bind(rid)
            .bind(idx)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = row else {
                return Err(crate::error::Error::NotFound {
                    id: format!("segment {idx} of recording {rid}"),
                });
            };
            let cur: Option<String> = row.try_get("speaker")?;
            if cur.as_deref() != Some(label_text.as_str()) {
                return Err(crate::error::Error::Internal(format!(
                    "segment {idx} is not currently speaker {label} (it is {})",
                    cur.as_deref().unwrap_or("unassigned")
                )));
            }
            spans.push((row.try_get("start_ms")?, row.try_get("end_ms")?));
        }
        for (&idx, (start_ms, end_ms)) in segment_idxs.iter().zip(&spans) {
            sqlx::query(
                "UPDATE transcript_segments SET speaker = ? WHERE recording_id = ? AND idx = ?",
            )
            .bind(&new_text)
            .bind(rid)
            .bind(idx)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE transcript_words SET speaker = ? \
                 WHERE recording_id = ? AND start_ms >= ? AND start_ms < ?",
            )
            .bind(&new_text)
            .bind(rid)
            .bind(*start_ms)
            .bind(*end_ms)
            .execute(&mut *tx)
            .await?;
        }
        Self::rebuild_transcript_markers_tx(&mut tx, recording_id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Rebuild the prose `transcript` text's `[Speaker N]:` markers from the
    /// current `transcript_segments`, inside an open transaction.
    ///
    /// The prose view and the rename modal read speaker structure from the stored
    /// `transcript` text, while the timeline and Synced views re-derive from
    /// segments, so after a label edit the text has to be re-rendered to agree.
    /// Only diarized transcripts (those that already carry `[Speaker N]:` markers)
    /// are rebuilt; a plain, un-diarized transcript has no markers to keep
    /// consistent and is left alone. Consecutive same-label segments are coalesced
    /// into one `[Speaker N]: <text>` turn, turns joined by `\n\n` — the same shape
    /// the diarizer emits. Segments with no speaker are skipped from marker
    /// emission (they can't appear in a diarized turn anyway).
    ///
    /// The rebuild uses each segment's stored `text` joined with a space, which
    /// reproduces the diarized turn text for the local and cloud paths that build
    /// both from one attribution. It deliberately leaves `original_transcript` and
    /// `clean_transcript` alone (machine truth is preserved) and doesn't set
    /// `user_edited` — a label correction isn't a transcript hand edit.
    async fn rebuild_transcript_markers_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        recording_id: &RecordingId,
    ) -> Result<()> {
        let current: Option<String> =
            sqlx::query_scalar("SELECT transcript FROM recordings WHERE id = ?")
                .bind(recording_id.as_str())
                .fetch_optional(&mut **tx)
                .await?
                .flatten();
        let Some(current) = current else {
            return Ok(()); // No transcript row/text — nothing to keep consistent.
        };
        // Only diarized prose carries the markers we own; leave plain text alone.
        if !current.contains("[Speaker ") {
            return Ok(());
        }
        let rows = sqlx::query(
            "SELECT text, speaker FROM transcript_segments WHERE recording_id = ? ORDER BY idx",
        )
        .bind(recording_id.as_str())
        .fetch_all(&mut **tx)
        .await?;
        let mut rebuilt = String::new();
        let mut current_label: Option<String> = None;
        for r in rows {
            let text: String = r.try_get("text")?;
            let speaker: Option<String> = r.try_get("speaker")?;
            let Some(label) = speaker else { continue };
            if current_label.as_deref() != Some(label.as_str()) {
                if !rebuilt.is_empty() {
                    rebuilt.push_str("\n\n");
                }
                rebuilt.push_str(&format!("[Speaker {label}]: "));
                current_label = Some(label);
            } else {
                rebuilt.push(' ');
            }
            rebuilt.push_str(text.trim());
        }
        sqlx::query(
            "UPDATE recordings SET transcript = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(&rebuilt)
        .bind(recording_id.as_str())
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Replace a recording's machine transcript words with a fresh set.
    ///
    /// The word-level twin of [`replace_segments`](Self::replace_segments):
    /// called by the pipeline after every transcribe/retranscribe, so the old
    /// rows are dropped first in the same transaction (a crash can't leave a
    /// half-replaced word timeline). An empty slice simply clears them — the
    /// normal state for a provider that emits no per-word timing.
    pub async fn replace_words(
        &self,
        recording_id: &RecordingId,
        words: &[TranscriptWord],
    ) -> Result<()> {
        self.replace_words_variant(recording_id, "raw", words).await
    }

    /// Replace one timing variant of a recording's words (`"raw"` machine-truth or
    /// `"cleaned"`, re-aligned to the post-cleanup text), leaving the other intact
    /// (TL-CONSISTENCY). The word-level twin of
    /// [`replace_segments_variant`](Self::replace_segments_variant).
    pub async fn replace_words_variant(
        &self,
        recording_id: &RecordingId,
        variant: &str,
        words: &[TranscriptWord],
    ) -> Result<()> {
        let table = words_table(variant);
        let mut tx = self.pool.begin().await?;
        sqlx::query(&format!("DELETE FROM {table} WHERE recording_id = ?"))
            .bind(recording_id.as_str())
            .execute(&mut *tx)
            .await?;
        for (idx, word) in words.iter().enumerate() {
            sqlx::query(&format!(
                "INSERT INTO {table} (recording_id, idx, start_ms, end_ms, text, speaker, confidence, leading_space) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            ))
            .bind(recording_id.as_str())
            .bind(idx as i64)
            .bind(word.start_ms)
            .bind(word.end_ms)
            .bind(&word.text)
            .bind(&word.speaker)
            .bind(word.confidence)
            .bind(word.leading_space)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A recording's machine transcript words in timeline order. Empty when the
    /// recording predates word capture or its provider returned no per-word
    /// timing — callers must treat "no words" as a normal state, not an error.
    pub async fn words_for(&self, recording_id: &RecordingId) -> Result<Vec<TranscriptWord>> {
        self.words_for_variant(recording_id, "raw").await
    }

    /// A recording's words for one timing `variant` (`"raw"` or `"cleaned"`), in
    /// timeline order. Empty when that variant has no rows — a normal state.
    pub async fn words_for_variant(
        &self,
        recording_id: &RecordingId,
        variant: &str,
    ) -> Result<Vec<TranscriptWord>> {
        let table = words_table(variant);
        let rows = sqlx::query(&format!(
            "SELECT start_ms, end_ms, text, speaker, confidence, leading_space FROM {table} \
             WHERE recording_id = ? ORDER BY idx"
        ))
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(TranscriptWord {
                    start_ms: r.try_get("start_ms")?,
                    end_ms: r.try_get("end_ms")?,
                    text: r.try_get("text")?,
                    // Stored as INTEGER (0/1); powers the Synced view's spacing.
                    leading_space: r.try_get::<i64, _>("leading_space")? != 0,
                    speaker: r.try_get("speaker")?,
                    confidence: r.try_get("confidence")?,
                })
            })
            .collect()
    }
}
