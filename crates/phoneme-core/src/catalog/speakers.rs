//! Speaker names, voiceprints, named voices, propagation, and recognition.

use super::*;

impl Catalog {
    /// Set (or clear) the custom display name for one diarized speaker label.
    ///
    /// `speaker_label` is the 1-based index from the transcript's `[Speaker N]`
    /// marker. A non-empty `name` upserts the mapping; a blank/whitespace-only
    /// `name` deletes it (the label reverts to the default "Speaker N"). The
    /// stored transcript is never touched — names are applied at display/export
    /// time — so renaming is fully reversible. The recording is expected to
    /// exist; a foreign-key violation surfaces as an error.
    pub async fn set_speaker_name(
        &self,
        recording_id: &RecordingId,
        speaker_label: i64,
        name: &str,
    ) -> Result<()> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            sqlx::query("DELETE FROM speaker_names WHERE recording_id = ? AND speaker_label = ?")
                .bind(recording_id.as_str())
                .bind(speaker_label)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO speaker_names (recording_id, speaker_label, name) VALUES (?, ?, ?) \
                 ON CONFLICT(recording_id, speaker_label) DO UPDATE SET name = excluded.name",
            )
            .bind(recording_id.as_str())
            .bind(speaker_label)
            .bind(trimmed)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Seed a default display name for a speaker label only if none exists yet —
    /// `INSERT ... ON CONFLICT DO NOTHING`, so an existing row is left untouched.
    ///
    /// This is the pipeline's "friendly default" path (the meeting mic track's
    /// label 1 → "You"). Unlike [`Self::set_speaker_name`] (the user/IPC rename
    /// path, an upsert), this never overwrites a name already on the row, so a user
    /// rename of that speaker survives a retranscribe or re-run that re-seeds the
    /// same default. The `name` is trimmed like in `set_speaker_name`; a
    /// blank/whitespace-only `name` is a no-op, since we never seed an empty
    /// default. The recording is expected to exist; a foreign-key violation
    /// surfaces as an error.
    pub async fn set_speaker_name_if_absent(
        &self,
        recording_id: &RecordingId,
        speaker_label: i64,
        name: &str,
    ) -> Result<()> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO speaker_names (recording_id, speaker_label, name) VALUES (?, ?, ?) \
             ON CONFLICT(recording_id, speaker_label) DO NOTHING",
        )
        .bind(recording_id.as_str())
        .bind(speaker_label)
        .bind(trimmed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All custom speaker names for a recording, ordered by speaker index. Empty
    /// when none have been set. Used to populate `Recording::speaker_names` and
    /// by the IPC layer so the frontend can map `[Speaker N]` → name at display
    /// and export time.
    pub async fn speaker_names_for(&self, recording_id: &RecordingId) -> Result<Vec<SpeakerName>> {
        let rows = sqlx::query(
            "SELECT speaker_label, name FROM speaker_names \
             WHERE recording_id = ? ORDER BY speaker_label",
        )
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(SpeakerName {
                    speaker_label: r.try_get("speaker_label")?,
                    name: r.try_get("name")?,
                })
            })
            .collect()
    }

    // ---- Speaker voiceprints — cross-recording named-speaker recognition (#9) ----

    /// Store (or refresh) the captured centroid for one speaker in a recording.
    /// The pipeline calls this for each labelled speaker after local diarization.
    /// An existing `named_voice_id` link is preserved (a re-transcribe refreshes
    /// the sample without un-enrolling), and the linked named voice is recomputed
    /// so its cached centroid tracks the new sample.
    ///
    /// `duration_ms` is the speaker's total speaking time in this recording — the
    /// duration-weight (roadmap V4) so a long, clean capture outvotes a brief one
    /// when the named voice is recomputed. Pass `0` when it isn't known; the
    /// weighted mean treats `0` as the equal-weight fallback (legacy behavior).
    pub async fn save_speaker_voiceprint(
        &self,
        recording_id: &str,
        speaker_label: i64,
        centroid: &[f32],
        duration_ms: i64,
    ) -> Result<()> {
        let json = serde_json::to_string(centroid)?;
        sqlx::query(
            "INSERT INTO speaker_voiceprints (recording_id, speaker_label, centroid, duration_ms) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(recording_id, speaker_label) DO UPDATE SET \
                 centroid = excluded.centroid, duration_ms = excluded.duration_ms",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .bind(&json)
        .bind(duration_ms)
        .execute(&self.pool)
        .await?;
        if let Some(nid) = self.named_voice_for(recording_id, speaker_label).await? {
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(())
    }

    /// The captured centroid for one speaker in a recording, if one exists.
    pub async fn speaker_voiceprint(
        &self,
        recording_id: &str,
        speaker_label: i64,
    ) -> Result<Option<Vec<f32>>> {
        let row = sqlx::query(
            "SELECT centroid FROM speaker_voiceprints \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_str::<Vec<f32>>(
                &r.try_get::<String, _>("centroid")?,
            )?)),
            None => Ok(None),
        }
    }

    /// The named-voice id a recording's speaker is enrolled under, if any.
    pub(crate) async fn named_voice_for(
        &self,
        recording_id: &str,
        speaker_label: i64,
    ) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT named_voice_id FROM speaker_voiceprints \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(r.try_get::<Option<String>, _>("named_voice_id")?),
            None => Ok(None),
        }
    }

    /// Enroll a recording's speaker into the named-voice library under `name` —
    /// the implicit-enrollment path, called whenever a speaker is named. Finds or
    /// creates the named voice by case-insensitive name, links the capture to it,
    /// and recomputes the library entry's cached centroid. Returns the named-voice
    /// id, or `None` when there's no captured voiceprint to enroll (e.g. a
    /// cloud-diarized recording) or the name is blank.
    pub async fn enroll_speaker(
        &self,
        recording_id: &str,
        speaker_label: i64,
        name: &str,
    ) -> Result<Option<String>> {
        let name = name.trim();
        if name.is_empty()
            || self
                .speaker_voiceprint(recording_id, speaker_label)
                .await?
                .is_none()
        {
            return Ok(None);
        }
        // What this capture was enrolled under before (if anything), so a re-name
        // — e.g. correcting a wrong suggestion — recomputes the old voice too.
        // Otherwise that voice keeps the moved sample's stale centroid and inflated
        // count (audit H2).
        let previous = self.named_voice_for(recording_id, speaker_label).await?;
        let id = self.find_or_create_named_voice(name).await?;
        sqlx::query(
            "UPDATE speaker_voiceprints SET named_voice_id = ? \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(&id)
        .bind(recording_id)
        .bind(speaker_label)
        .execute(&self.pool)
        .await?;
        // Dismissals are name-agnostic: a speaker dismissed before the right
        // voice existed in the library left a row that would suppress every
        // future suggestion for this (recording, label). Naming it is an
        // explicit identification, so clear that row — once a matching voice
        // exists the speaker can be recognized/renamed again (audit M9).
        sqlx::query(
            "DELETE FROM dismissed_speaker_suggestions \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .execute(&self.pool)
        .await?;
        self.recompute_named_centroid(&id).await?;
        if let Some(prev) = previous {
            if prev != id {
                self.recompute_named_centroid(&prev).await?;
            }
        }
        Ok(Some(id))
    }

    /// Un-enroll a recording's speaker from its named voice (keeps the raw
    /// capture; recomputes the formerly-linked voice). No-op when not enrolled.
    pub async fn unenroll_speaker(&self, recording_id: &str, speaker_label: i64) -> Result<()> {
        if let Some(nid) = self.named_voice_for(recording_id, speaker_label).await? {
            sqlx::query(
                "UPDATE speaker_voiceprints SET named_voice_id = NULL \
                 WHERE recording_id = ? AND speaker_label = ?",
            )
            .bind(recording_id)
            .bind(speaker_label)
            .execute(&self.pool)
            .await?;
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(())
    }

    /// Find a named voice by case-insensitive name, creating an empty one (no
    /// samples yet) if none matches. Returns its id.
    async fn find_or_create_named_voice(&self, name: &str) -> Result<String> {
        // Atomic find-or-create: the INSERT is a single (serialized) write that
        // fires only when no case-insensitive match exists, so two enrollments
        // racing under the same name can't create duplicate library entries
        // (audit M4). Then read back the existing-or-just-created id.
        let id = format!("nv_{}", RecordingId::new().as_str());
        // A forgotten (soft-deleted) voice with this name doesn't count as a match:
        // re-using a forgotten name creates a fresh live voice rather than silently
        // reviving a tombstoned one (undo is the explicit revive path).
        sqlx::query(
            "INSERT INTO named_voiceprints (id, name, centroid, samples) \
             SELECT ?, ?, '[]', 0 \
             WHERE NOT EXISTS (SELECT 1 FROM named_voiceprints \
                 WHERE name = ? COLLATE NOCASE AND deleted_at IS NULL)",
        )
        .bind(&id)
        .bind(name)
        .bind(name)
        .execute(&self.pool)
        .await?;
        let resolved: String = sqlx::query_scalar(
            "SELECT id FROM named_voiceprints \
             WHERE name = ? COLLATE NOCASE AND deleted_at IS NULL LIMIT 1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(resolved)
    }

    /// Drop clear outliers from a named voice's linked captures before averaging.
    ///
    /// Each sample is `(centroid, duration_weight)`. Outliers are judged purely on
    /// centroid geometry — a long sample of the wrong speaker is still the wrong
    /// speaker — and each survivor keeps its weight so the caller can take a
    /// duration-weighted mean over what's left (roadmap V4).
    ///
    /// With fewer than 4 samples there's too little signal to judge, so the input
    /// is returned unchanged. At 4 or more, a provisional (unweighted) mean is
    /// taken, then any capture whose cosine to that mean is below either a hard
    /// floor (`0.2`, almost certainly a different speaker) or `mean - 2*stddev` (a
    /// statistical outlier) is removed. If pruning would drop everything — a
    /// degenerate cutoff, or a provisional mean that can't be computed (e.g. mixed
    /// dimensions) — the originals are kept so the voice never silently empties.
    fn drop_centroid_outliers(samples: Vec<(Vec<f32>, f64)>) -> Vec<(Vec<f32>, f64)> {
        const MIN_SAMPLES_TO_PRUNE: usize = 4;
        const HARD_FLOOR: f32 = 0.2;

        if samples.len() < MIN_SAMPLES_TO_PRUNE {
            return samples;
        }
        // The provisional mean used for outlier detection is unweighted: duration
        // doesn't decide who counts as an outlier, only how much a survivor
        // contributes to the final template.
        let centroids: Vec<Vec<f32>> = samples.iter().map(|(c, _)| c.clone()).collect();
        let provisional = match crate::voiceprint::mean_centroid(&centroids) {
            Some(m) => m,
            None => return samples, // mixed dims etc.: can't judge, keep all
        };
        let sims: Vec<f32> = samples
            .iter()
            .map(|(c, _)| crate::voiceprint::cosine_similarity(c, &provisional))
            .collect();
        let n = sims.len() as f32;
        let mean: f32 = sims.iter().sum::<f32>() / n;
        let var: f32 = sims.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / n;
        let cutoff = (mean - 2.0 * var.sqrt()).max(HARD_FLOOR);

        let kept: Vec<(Vec<f32>, f64)> = samples
            .iter()
            .zip(sims.iter())
            .filter(|(_, &sim)| sim >= cutoff)
            .map(|((c, w), _)| (c.clone(), *w))
            .collect();
        // A degenerate cutoff — every sample identical, so var is 0 and float
        // jitter drops the whole set — must not empty the voice; fall back to the
        // full sample set in that case.
        if kept.is_empty() {
            return samples;
        }
        kept
    }

    /// Recompute a named voice's cached centroid and sample count from its linked
    /// captures: the duration-weighted, L2-normalized mean (roadmap V4) over the
    /// survivors of [`Self::drop_centroid_outliers`]. Weighting is applied after
    /// outlier rejection, so a long sample only counts more once it's already been
    /// judged a genuine member of the cluster — it can't drag in a wrong-speaker
    /// capture just by being lengthy. Legacy captures with `duration_ms = 0` fall
    /// back to equal weighting, so a library built before this feature recomputes
    /// to the same centroid until new, duration-bearing captures arrive. A voice
    /// with no remaining captures gets an empty centroid and zero samples — it
    /// never matches, but the entry stays until explicitly forgotten.
    pub(crate) async fn recompute_named_centroid(&self, named_voice_id: &str) -> Result<()> {
        let rows = sqlx::query(
            "SELECT centroid, duration_ms FROM speaker_voiceprints WHERE named_voice_id = ?",
        )
        .bind(named_voice_id)
        .fetch_all(&self.pool)
        .await?;
        let mut samples: Vec<(Vec<f32>, f64)> = Vec::with_capacity(rows.len());
        for r in rows {
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            let duration_ms: i64 = r.try_get("duration_ms")?;
            samples.push((centroid, duration_ms as f64));
        }
        // With enough captures, prune clear outliers before the final mean so one
        // mis-assigned sample (a wrong-speaker capture named into this voice) can't
        // drag the template off the real speaker (audit M7). Below the threshold
        // every sample counts — too few to tell signal from noise.
        let kept = Self::drop_centroid_outliers(samples);
        let mean = crate::voiceprint::weighted_mean_centroid(&kept).unwrap_or_default();
        let json = serde_json::to_string(&mean)?;
        sqlx::query(
            "UPDATE named_voiceprints SET centroid = ?, samples = ?, updated_at = datetime('now') \
             WHERE id = ?",
        )
        .bind(&json)
        .bind(kept.len() as i64)
        .bind(named_voice_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The named-voice library (id, name, sample count), most-recently-updated
    /// first — for the Speaker Library manager.
    pub async fn list_named_voices(&self) -> Result<Vec<NamedVoice>> {
        let rows = sqlx::query(
            "SELECT id, name, samples FROM named_voiceprints \
             WHERE deleted_at IS NULL \
             ORDER BY updated_at DESC, name COLLATE NOCASE ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(NamedVoice {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    samples: r.try_get::<i64, _>("samples")? as u32,
                })
            })
            .collect()
    }

    /// Match a probe centroid against the named-voice library, returning the best
    /// `(NamedVoice, score)` at or above `threshold`. Voices with no samples
    /// (empty centroid) never match. Used by recognition to suggest a name.
    pub async fn recognize_voice(
        &self,
        probe: &[f32],
        threshold: f32,
    ) -> Result<Option<(NamedVoice, f32)>> {
        let rows = sqlx::query(
            "SELECT id, name, samples, centroid FROM named_voiceprints \
             WHERE deleted_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut voices: Vec<NamedVoice> = Vec::with_capacity(rows.len());
        let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(rows.len());
        for r in rows {
            let samples: i64 = r.try_get("samples")?;
            if samples <= 0 {
                continue;
            }
            let id: String = r.try_get("id")?;
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            // A centroid whose dimension differs from the probe came from a
            // different embedding model. Cosine would silently return 0.0, so such
            // a library would go quietly unmatched; skip it with a warning instead
            // (audit L).
            if centroid.len() != probe.len() {
                tracing::warn!(
                    id = %id,
                    dim = centroid.len(),
                    probe_dim = probe.len(),
                    "skipping named voice: centroid dimension mismatch (cross-model library)"
                );
                continue;
            }
            voices.push(NamedVoice {
                id,
                name: r.try_get("name")?,
                samples: samples as u32,
            });
            centroids.push(centroid);
        }
        Ok(crate::voiceprint::best_match(probe, &centroids, threshold)
            .map(|(i, score)| (voices[i].clone(), score)))
    }

    /// Rename a named voice (no-op on a blank name or unknown id).
    pub async fn rename_named_voice(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE named_voiceprints SET name = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(name)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Merge `from_id` into `into_id`: re-point all of `from`'s captures onto
    /// `into`, recompute `into`'s centroid, and delete `from`. Returns whether a
    /// merge happened (both ids exist and differ).
    pub async fn merge_named_voices(&self, from_id: &str, into_id: &str) -> Result<bool> {
        if from_id == into_id {
            return Ok(false);
        }
        let both: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM named_voiceprints WHERE id IN (?, ?)")
                .bind(from_id)
                .bind(into_id)
                .fetch_one(&self.pool)
                .await?;
        if both < 2 {
            return Ok(false);
        }
        sqlx::query("UPDATE speaker_voiceprints SET named_voice_id = ? WHERE named_voice_id = ?")
            .bind(into_id)
            .bind(from_id)
            .execute(&self.pool)
            .await?;
        // Drop any undo-log rows keyed by the absorbed voice. The FK on
        // forgotten_voiceprint_links already cascades these on the hard delete
        // below; the explicit DELETE is redundant but makes the intent obvious.
        sqlx::query("DELETE FROM forgotten_voiceprint_links WHERE named_voice_id = ?")
            .bind(from_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM named_voiceprints WHERE id = ?")
            .bind(from_id)
            .execute(&self.pool)
            .await?;
        self.recompute_named_centroid(into_id).await?;
        Ok(true)
    }

    /// Forget a named voice — reversibly (roadmap V5). The library entry is
    /// soft-deleted (`deleted_at` stamped, not dropped), its captures are unlinked
    /// (the raw per-recording voiceprints stay), and which captures were unlinked
    /// is recorded in the undo log so [`Self::undo_forget`] can re-link exactly
    /// those rows. A tombstoned voice is invisible to listing and recognition just
    /// like the old hard delete, but recoverable. Returns whether a live row was
    /// forgotten (false for an unknown or already-forgotten id). Idempotent: a
    /// second forget of the same id is a no-op.
    pub async fn forget_named_voice(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        // Only a live voice can be forgotten; this guards idempotency and gives us
        // the rows_affected to report.
        let stamped = sqlx::query(
            "UPDATE named_voiceprints SET deleted_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if stamped.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        // Snapshot which captures are about to be unlinked, so undo can restore the
        // exact set (a stale log from a prior forget of this id is cleared first).
        sqlx::query("DELETE FROM forgotten_voiceprint_links WHERE named_voice_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO forgotten_voiceprint_links (named_voice_id, recording_id, speaker_label) \
             SELECT named_voice_id, recording_id, speaker_label FROM speaker_voiceprints \
             WHERE named_voice_id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE speaker_voiceprints SET named_voice_id = NULL WHERE named_voice_id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Undo a [`Self::forget_named_voice`]: clear the tombstone, re-link the
    /// captures the forget unlinked (from the undo log), recompute the cached
    /// centroid, and clear the log. Returns whether a forgotten voice was restored
    /// (false for an unknown or not-currently-forgotten id). Idempotent on a
    /// live id.
    pub async fn undo_forget(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let revived = sqlx::query(
            "UPDATE named_voiceprints SET deleted_at = NULL \
             WHERE id = ? AND deleted_at IS NOT NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if revived.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        // Re-link only captures still currently unlinked — a capture re-named onto
        // a different voice since the forget keeps its newer assignment (don't
        // clobber a deliberate re-enrollment).
        sqlx::query(
            "UPDATE speaker_voiceprints SET named_voice_id = ? \
             WHERE named_voice_id IS NULL AND (recording_id, speaker_label) IN \
                 (SELECT recording_id, speaker_label FROM forgotten_voiceprint_links \
                  WHERE named_voice_id = ?)",
        )
        .bind(id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM forgotten_voiceprint_links WHERE named_voice_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        // Re-derive the cached centroid + count from the re-linked captures.
        self.recompute_named_centroid(id).await?;
        Ok(true)
    }

    // ---- Name propagation — back-fill a name onto past recordings (V5) ----

    /// Find the unnamed speakers in other recordings whose voiceprint matches a
    /// named voice — the back-fill candidates for naming propagation (V5).
    ///
    /// Scans every `speaker_voiceprints` row with `named_voice_id IS NULL` (a
    /// captured-but-never-named speaker), in any recording other than ones already
    /// linked to this voice, and scores its centroid against the named voice's
    /// cached centroid with the same scorer the recognizer uses: raw cosine under
    /// [`ScoreNorm::Off`](crate::voiceprint::ScoreNorm::Off), or the z-score under
    /// `s_norm`/`as_norm` with the live named-voice library as the cohort. A row is
    /// a candidate when it clears `threshold`, interpreted on the same scale the
    /// chosen `mode` uses (exactly as `recognize_speakers_for`). Already-named
    /// speakers are never candidates — only `named_voice_id IS NULL` rows are
    /// scanned — so propagation can only add a name, never overwrite one. Results
    /// are ordered by score, highest first.
    ///
    /// Returns empty when the voice is unknown, forgotten, or has no centroid yet.
    pub async fn propagation_candidates(
        &self,
        named_voice_id: &str,
        threshold: f32,
        mode: crate::voiceprint::ScoreNorm,
    ) -> Result<Vec<PropagationCandidate>> {
        // The live library is both the cohort for normalization and the source of
        // the target centroid. A forgotten or sample-less voice isn't here, so it
        // yields no candidates.
        let library = self.named_voice_centroids().await?;
        let target_idx = match library.iter().position(|(v, _)| v.id == named_voice_id) {
            Some(i) => i,
            None => return Ok(Vec::new()),
        };
        let cohort: Vec<Vec<f32>> = library.iter().map(|(_, c)| c.clone()).collect();

        // Every unnamed capture: not enrolled (`named_voice_id IS NULL`) and with
        // no display name (`speaker_names`). A speaker can carry a display name
        // without being enrolled — e.g. the pipeline's "You" default, or a name set
        // on a cloud-diarized recording with no voiceprint — and propagation must
        // never overwrite such a name. The `IS NULL` filter is per-speaker (the PK
        // is `(recording_id, speaker_label)`), so a speaker already enrolled under
        // this — or any — voice is excluded directly, while a second, still-unnamed
        // speaker of the same voice in the same recording is still admitted. There
        // is deliberately no recording-wide exclusion: that would drop that second
        // speaker. And there's no per-speaker enrolled-guard: it would self-join the
        // PK against an `IS NULL` row, which is always vacuously true — a no-op that
        // only misleads.
        let rows = sqlx::query(
            "SELECT sv.recording_id AS recording_id, sv.speaker_label AS speaker_label, \
                    sv.centroid AS centroid \
             FROM speaker_voiceprints sv \
             WHERE sv.named_voice_id IS NULL \
               AND NOT EXISTS (SELECT 1 FROM speaker_names sn \
                     WHERE sn.recording_id = sv.recording_id \
                       AND sn.speaker_label = sv.speaker_label)",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out: Vec<PropagationCandidate> = Vec::new();
        for r in rows {
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            // A dimension mismatch (cross-model capture) scores cosine 0.0 and
            // won't clear any sane threshold — the same skip the recognizer makes.
            let score = crate::voiceprint::normalized_score(&centroid, &cohort, target_idx, mode);
            if score >= threshold {
                let recording_id: String = r.try_get("recording_id")?;
                let speaker_label: i64 = r.try_get("speaker_label")?;
                let rid = match RecordingId::parse(recording_id) {
                    Some(rid) => rid,
                    None => continue, // a malformed id can't be a real recording
                };
                out.push(PropagationCandidate {
                    recording_id: rid,
                    speaker_label,
                    score,
                });
            }
        }
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out)
    }

    /// Back-fill a named voice onto the given `(recording_id, speaker_label)`
    /// targets (V5) — the apply half of propagation.
    ///
    /// For each target it does exactly what a normal naming does: set the
    /// per-recording display name (`speaker_names`, the same row
    /// [`Self::set_speaker_name`] writes — the transcript's `[Speaker N]` markers
    /// are mapped to it at display/export time, never rewritten in place) and
    /// enroll the capture into the library so the voice gets stronger. The voice's
    /// own name is read from the library by id.
    ///
    /// Safety and idempotency: a target whose speaker is already named is skipped
    /// (we never overwrite a name), as is a target with no captured voiceprint or
    /// one already enrolled under this voice. Re-running with the same targets does
    /// no duplicate work. Returns the targets actually back-filled, so callers can
    /// refresh exactly those recordings rather than every candidate.
    ///
    /// Best-effort per target: the voice must exist and be live (a forgotten voice
    /// back-fills nothing).
    pub async fn apply_propagation(
        &self,
        named_voice_id: &str,
        targets: &[(RecordingId, i64)],
    ) -> Result<Vec<(RecordingId, i64)>> {
        // Resolve the name from the live library; a forgotten or unknown voice
        // can't be propagated.
        let name: Option<String> = sqlx::query_scalar(
            "SELECT name FROM named_voiceprints WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(named_voice_id)
        .fetch_optional(&self.pool)
        .await?;
        let name = match name {
            Some(n) => n,
            None => return Ok(Vec::new()),
        };

        let mut applied: Vec<(RecordingId, i64)> = Vec::new();
        for (recording_id, speaker_label) in targets {
            // Never overwrite an existing name — only back-fill an unnamed speaker.
            let already_named: Option<String> = sqlx::query_scalar(
                "SELECT name FROM speaker_names WHERE recording_id = ? AND speaker_label = ?",
            )
            .bind(recording_id.as_str())
            .bind(speaker_label)
            .fetch_optional(&self.pool)
            .await?;
            if already_named.is_some() {
                continue;
            }
            // Skip a capture already enrolled under this voice (idempotent re-run).
            if self
                .named_voice_for(recording_id.as_str(), *speaker_label)
                .await?
                .as_deref()
                == Some(named_voice_id)
            {
                continue;
            }
            // Apply the display name (same write as set_speaker_name) and enroll the
            // capture. enroll_speaker is a no-op when no voiceprint was captured.
            self.set_speaker_name(recording_id, *speaker_label, &name)
                .await?;
            // Don't `?` the enroll: a failure after the name was written would
            // leave a display name with no enrollment. Match on it so we can roll
            // the name back.
            match self
                .enroll_speaker(recording_id.as_str(), *speaker_label, &name)
                .await
            {
                Ok(Some(_)) => applied.push((recording_id.clone(), *speaker_label)),
                Ok(None) => {
                    // No voiceprint to enroll — undo the name we just wrote so a target
                    // with no capture isn't half-applied (display-named but unenrolled).
                    self.set_speaker_name(recording_id, *speaker_label, "")
                        .await?;
                }
                Err(e) => {
                    // Enrollment failed — roll back the name we just wrote, then
                    // propagate the error (best-effort undo; the enroll error wins).
                    let _ = self
                        .set_speaker_name(recording_id, *speaker_label, "")
                        .await;
                    return Err(e);
                }
            }
        }
        Ok(applied)
    }

    /// All captured voiceprints for a recording, as `(speaker_label, centroid)`.
    pub async fn speaker_voiceprints_for(
        &self,
        recording_id: &str,
    ) -> Result<Vec<(i64, Vec<f32>)>> {
        let rows = sqlx::query(
            "SELECT speaker_label, centroid FROM speaker_voiceprints \
             WHERE recording_id = ? ORDER BY speaker_label",
        )
        .bind(recording_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let label: i64 = r.try_get("speaker_label")?;
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            out.push((label, centroid));
        }
        Ok(out)
    }

    /// Mark a recognized-speaker suggestion dismissed so it isn't offered again.
    pub async fn dismiss_speaker_suggestion(
        &self,
        recording_id: &str,
        speaker_label: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO dismissed_speaker_suggestions (recording_id, speaker_label) \
             VALUES (?, ?)",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One column of i64s from a recording-scoped table, as a set.
    async fn label_set(
        &self,
        query: &str,
        recording_id: &str,
    ) -> Result<std::collections::HashSet<i64>> {
        let rows = sqlx::query(query)
            .bind(recording_id)
            .fetch_all(&self.pool)
            .await?;
        let mut set = std::collections::HashSet::with_capacity(rows.len());
        for r in rows {
            set.insert(r.try_get::<i64, _>("speaker_label")?);
        }
        Ok(set)
    }

    /// The named-voice library as `(NamedVoice, centroid)` pairs, skipping
    /// empty entries (no samples). Used by recognition to score every captured
    /// speaker against the whole library at once.
    async fn named_voice_centroids(&self) -> Result<Vec<(NamedVoice, Vec<f32>)>> {
        let rows = sqlx::query(
            "SELECT id, name, samples, centroid FROM named_voiceprints \
             WHERE deleted_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let samples: i64 = r.try_get("samples")?;
            if samples <= 0 {
                continue;
            }
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            out.push((
                NamedVoice {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    samples: samples as u32,
                },
                centroid,
            ));
        }
        Ok(out)
    }

    /// On-demand named-speaker recognition for a recording (#9): each captured
    /// speaker that has no name yet and hasn't been dismissed is matched against
    /// the named-voice library, and the surviving suggestions are returned. This
    /// reads the live library, so a voice named after this recording was
    /// transcribed is still suggested here.
    ///
    /// Assignment is one-to-one: the full captured-speaker × named-voice cosine
    /// matrix is scored once, then the best pairs are taken in descending score so
    /// no two speakers in the same recording can be handed the same name (audit
    /// H2). A pair is only emitted when it clears `threshold` and beats that
    /// speaker's own second-best candidate by `MARGIN` — an ambiguous speaker (two
    /// library voices nearly tied) is left unknown rather than guessed. The result
    /// holds at most one suggestion per captured speaker and per name, ordered by
    /// `speaker_label`.
    pub async fn recognize_speakers_for(
        &self,
        recording_id: &str,
        threshold: f32,
        mode: crate::voiceprint::ScoreNorm,
    ) -> Result<Vec<SpeakerSuggestion>> {
        let captured = self.speaker_voiceprints_for(recording_id).await?;
        if captured.is_empty() {
            return Ok(Vec::new());
        }
        let named = self
            .label_set(
                "SELECT speaker_label FROM speaker_names WHERE recording_id = ?",
                recording_id,
            )
            .await?;
        let dismissed = self
            .label_set(
                "SELECT speaker_label FROM dismissed_speaker_suggestions WHERE recording_id = ?",
                recording_id,
            )
            .await?;
        // Only speakers still eligible for a suggestion (un-named, un-dismissed).
        let probes: Vec<(i64, Vec<f32>)> = captured
            .into_iter()
            .filter(|(label, _)| !named.contains(label) && !dismissed.contains(label))
            .collect();
        if probes.is_empty() {
            return Ok(Vec::new());
        }
        let library = self.named_voice_centroids().await?;
        Ok(Self::assign_speakers(&probes, &library, threshold, mode))
    }

    /// Margin a winning match must beat the speaker's second-best by — below
    /// this gap the two candidates are too close to call, so no name is offered.
    const MARGIN: f32 = 0.05;

    /// One-to-one greedy assignment of captured speakers to named voices.
    ///
    /// Builds the score matrix, then repeatedly takes the highest remaining
    /// `(speaker, voice)` cell whose speaker and voice are both still free, whose
    /// score clears `threshold`, and whose score beats that speaker's second-best
    /// over the whole library by `MARGIN`. Each speaker and each named voice is
    /// used at most once. Output is sorted by `speaker_label` for a stable
    /// suggestion order.
    ///
    /// `mode` selects the scorer (V2): [`ScoreNorm::Off`](crate::voiceprint::ScoreNorm::Off)
    /// uses raw cosine, and `threshold` is the cosine bar; `s_norm`/`as_norm`
    /// z-score each probe against the rest of the library (the cohort), and
    /// `threshold` is then a z-score bar. The library serves as the cohort for both
    /// probe-side and (AS-norm) target-side normalization.
    fn assign_speakers(
        probes: &[(i64, Vec<f32>)],
        library: &[(NamedVoice, Vec<f32>)],
        threshold: f32,
        mode: crate::voiceprint::ScoreNorm,
    ) -> Vec<SpeakerSuggestion> {
        if library.is_empty() {
            return Vec::new();
        }
        // Cohort = the library centroids; normalize each probe against the other
        // library voices. With ScoreNorm::Off this reduces to raw cosine, so the
        // default path is unchanged. A dimension mismatch yields cosine 0.0
        // (treated as no signal), matching `recognize_voice`'s skip; the margin
        // test below keeps such a row from ever winning.
        let cohort: Vec<Vec<f32>> = library.iter().map(|(_, c)| c.clone()).collect();
        let scores: Vec<Vec<f32>> = probes
            .iter()
            .map(|(_, probe)| {
                (0..cohort.len())
                    .map(|ti| crate::voiceprint::normalized_score(probe, &cohort, ti, mode))
                    .collect()
            })
            .collect();

        // Each speaker's second-best score across the whole library, used for the
        // ambiguity margin even after a voice is claimed by another speaker.
        let second_best: Vec<f32> = scores
            .iter()
            .map(|row| {
                let mut top2 = [f32::NEG_INFINITY, f32::NEG_INFINITY];
                for &s in row {
                    if s > top2[0] {
                        top2[1] = top2[0];
                        top2[0] = s;
                    } else if s > top2[1] {
                        top2[1] = s;
                    }
                }
                if top2[1].is_finite() {
                    top2[1]
                } else {
                    f32::NEG_INFINITY // only one candidate → no rival to clear
                }
            })
            .collect();

        let mut speaker_taken = vec![false; probes.len()];
        let mut voice_taken = vec![false; library.len()];
        let mut out: Vec<SpeakerSuggestion> = Vec::new();

        // Greedy: pick the globally-highest free cell each round until none qualify.
        loop {
            let mut best: Option<(usize, usize, f32)> = None;
            for (si, row) in scores.iter().enumerate() {
                if speaker_taken[si] {
                    continue;
                }
                for (vi, &score) in row.iter().enumerate() {
                    if voice_taken[vi] || score < threshold {
                        continue;
                    }
                    // Must clear the speaker's own runner-up by a margin; if not,
                    // it's ambiguous, so leave this speaker unknown.
                    let runner_up = second_best[si];
                    if runner_up.is_finite() && score < runner_up + Self::MARGIN {
                        continue;
                    }
                    if best.is_none_or(|(_, _, b)| score > b) {
                        best = Some((si, vi, score));
                    }
                }
            }
            match best {
                Some((si, vi, score)) => {
                    speaker_taken[si] = true;
                    voice_taken[vi] = true;
                    out.push(SpeakerSuggestion {
                        speaker_label: probes[si].0,
                        name: library[vi].0.name.clone(),
                        named_voice_id: library[vi].0.id.clone(),
                        score,
                    });
                }
                None => break,
            }
        }
        out.sort_by_key(|s| s.speaker_label);
        out
    }
}
