//! Meeting + period digest: merged-transcript assembly, the meeting recipe
//! executor, and the cross-recording period rollup. Split out of pipeline.rs;
//! `use super::*` pulls in the shared helpers (generate_summary_with,
//! summary_llm_config, run_llm_stage, …) and imports that live on the parent.

use super::*;

/// Human source label for one meeting track, used when assembling the merged
/// transcript the digest reads. Mirrors the frontend `sourceFor` mapping
/// (mic → Microphone, system → System audio) so the digest's view of who-said-what
/// matches the merged meeting view.
fn track_source_label(track: Option<&str>) -> &'static str {
    match track {
        Some("mic") => "Microphone",
        Some("system") => "System audio",
        _ => "Track",
    }
}

/// Assemble a single merged transcript spanning every track of a meeting, for the
/// whole-meeting digest. Tracks are ordered like the merged meeting view (by
/// `started_at`, ties broken by track name so "mic" leads "system"); each track's
/// stored live transcript is prefixed with its source label so the LLM can tell
/// the local speaker from the other party. Tracks with no transcript yet (still
/// transcribing, or failed) contribute nothing. Returns an empty string when no
/// track has any transcript — the caller treats that as "nothing to digest".
///
/// This is the coarse by-source merge (the same structure
/// `frontend/.../mergeMeeting.ts` falls back to), not the chronological segment
/// interleave: for an LLM digest the per-track text with source labels carries the
/// needed structure without depending on segment timing being present.
pub(crate) fn assemble_meeting_transcript(tracks: &[phoneme_core::Recording]) -> String {
    let mut ordered: Vec<&phoneme_core::Recording> = tracks
        .iter()
        .filter(|r| {
            r.transcript
                .as_deref()
                .map(str::trim)
                .is_some_and(|t| !t.is_empty())
        })
        .collect();
    ordered.sort_by(|a, b| {
        a.started_at.cmp(&b.started_at).then_with(|| {
            a.track
                .as_deref()
                .unwrap_or("")
                .cmp(b.track.as_deref().unwrap_or(""))
        })
    });
    ordered
        .iter()
        .map(|r| {
            let label = track_source_label(r.track.as_deref());
            let text = r.transcript.as_deref().unwrap_or("").trim();
            format!("=== {label} ===\n{text}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Generate an LLM digest of a whole meeting: assemble every track's transcript
/// into one merged document ([`assemble_meeting_transcript`]) and run it through
/// the same summary provider + streaming/persistence path the per-recording
/// summary uses ([`generate_summary_with`]), but with the meeting-scope
/// [`MEETING_DIGEST_PROMPT`]. Returns `(digest, model)` on success or a
/// human-readable reason on failure (reaches the UI toast verbatim). The
/// `LlmActivity`/skip events are keyed on `event_id` (the meeting's first track),
/// since the activity stream is per-recording; the result is stored against the
/// meeting, not the track.
#[allow(dead_code)] // retained primitive: superseded by run_meeting_recipe, kept for tests + as the documented single-step digest path
pub(crate) async fn generate_meeting_digest(
    state: &AppState,
    cfg: &Config,
    event_id: &RecordingId,
    tracks: &[phoneme_core::Recording],
) -> std::result::Result<(String, String), String> {
    let merged = assemble_meeting_transcript(tracks);
    if merged.trim().is_empty() {
        return Err("no transcribed tracks yet — nothing to digest".into());
    }
    let endpoint_hint = cfg.summary.api_url.trim();
    let endpoint_hint = (!endpoint_hint.is_empty()).then(|| endpoint_hint.to_string());
    // Reuse the summary connection (provider/model/key/url) but the meeting-scope
    // prompt. `generate_summary_with` gives identical events + skip/empty/error
    // classification + recorded model as the per-recording summary.
    generate_summary_with(
        state,
        event_id,
        &merged,
        summary_llm_config(cfg),
        MEETING_DIGEST_PROMPT,
        endpoint_hint.as_deref(),
    )
    .await
}

/// One resolved meeting-scope step, ready to dispatch by [`run_meeting_recipe`].
/// The meeting executor is deliberately narrower than the per-track one: only an
/// Enrichment writing the meeting digest and a Hook side-effect make sense over a
/// merged transcript (there is no single canonical transcript to rewrite back to,
/// so Transform/FillerRemoval are warn-and-skipped before they ever become a
/// `ResolvedMeetingStep`).
enum ResolvedMeetingStep {
    /// Run the summary primitive over the merged transcript with this entry's
    /// resolved LLM config + prompt, writing the meeting digest. The meeting
    /// counterpart of [`ResolvedStep::Summary`].
    Digest {
        llm_cfg: LlmPostProcessConfig,
        prompt: String,
    },
    /// A side-effect step (a `Hook` Playbook entry) run once with the merged
    /// transcript. The meeting counterpart of [`ResolvedStep::Hook`].
    Hook {
        hook: phoneme_core::config::PlaybookHook,
    },
}

/// Resolve the configured meeting recipe (`cfg.meeting_recipe_id`) into an ordered
/// list of meeting-scope steps. Mirrors [`resolve_recipe`], but for the
/// meeting-template path:
///
/// - An empty `meeting_recipe_id` (the default), a missing recipe (a user deleted
///   the one a setting still names), or a non-`Meeting`-scope recipe **all fall
///   back to the built-in single-step digest** (the [`MEETING_DIGEST_PROMPT`] over
///   the summary connection) — so behaviour is never worse than today and the
///   function never panics.
/// - Each step id maps to its Playbook entry. An Enrichment whose target is
///   `meeting_digest` or `summary` becomes a [`ResolvedMeetingStep::Digest`]; a
///   `Hook` entry with a command/webhook becomes a [`ResolvedMeetingStep::Hook`].
/// - `Transform` / `FillerRemoval` and unsupported Enrichment targets are
///   warn-and-skipped: there is no single merged transcript to rewrite, and no
///   keyed-by-meeting store for other targets in v1.
fn resolve_meeting_recipe(cfg: &Config) -> Vec<ResolvedMeetingStep> {
    use phoneme_core::config::{PlaybookKind, RecipeScope};

    // The built-in fallback: the digest prompt over the summary connection. Used
    // whenever no usable meeting recipe is configured.
    let fallback = || {
        vec![ResolvedMeetingStep::Digest {
            llm_cfg: summary_llm_config(cfg),
            prompt: MEETING_DIGEST_PROMPT.to_string(),
        }]
    };

    let id = cfg.meeting_recipe_id.trim();
    if id.is_empty() {
        return fallback();
    }

    let seeded = phoneme_core::config::default_recipes();
    let recipe = cfg
        .recipes
        .iter()
        .find(|r| r.id == id)
        .or_else(|| seeded.iter().find(|r| r.id == id));
    let Some(recipe) = recipe else {
        tracing::warn!(
            recipe = %id,
            "meeting_recipe_id names no recipe; falling back to the built-in meeting digest"
        );
        return fallback();
    };
    if recipe.scope != RecipeScope::Meeting {
        tracing::warn!(
            recipe = %id,
            "meeting_recipe_id names a non-meeting-scope recipe; falling back to the built-in meeting digest"
        );
        return fallback();
    }

    let mut steps = Vec::with_capacity(recipe.steps.len());
    for step_id in &recipe.steps {
        let Some(entry) = cfg.playbook.iter().find(|e| &e.id == step_id) else {
            tracing::warn!(step = %step_id, "meeting recipe references a missing Playbook entry; skipping");
            continue;
        };
        match entry.kind {
            PlaybookKind::Enrichment => {
                let target = entry.target.trim();
                if target == "meeting_digest" || target == "summary" {
                    steps.push(ResolvedMeetingStep::Digest {
                        llm_cfg: meeting_entry_llm_config(cfg, &entry.llm),
                        prompt: entry.llm.prompt.clone(),
                    });
                } else {
                    tracing::warn!(
                        target = %target,
                        "meeting recipe enrichment target is not meeting-scope (only `meeting_digest` is supported in v1); skipping"
                    );
                }
            }
            PlaybookKind::Hook => {
                if entry.hook.command.trim().is_empty() && entry.hook.webhook_url.trim().is_empty()
                {
                    tracing::warn!(step = %step_id, "meeting Hook entry has no command or webhook; skipping");
                } else {
                    steps.push(ResolvedMeetingStep::Hook {
                        hook: entry.hook.clone(),
                    });
                }
            }
            PlaybookKind::Transform | PlaybookKind::FillerRemoval => {
                tracing::warn!(
                    step = %step_id,
                    "meeting recipe Transform/FillerRemoval step skipped — there is no single merged transcript to rewrite at meeting scope"
                );
            }
        }
    }

    // A recipe whose only steps were all skipped (e.g. all Transforms) still must
    // produce a digest — fall back rather than silently produce nothing.
    if steps.is_empty() {
        tracing::warn!(
            recipe = %id,
            "meeting recipe resolved to no runnable steps; falling back to the built-in meeting digest"
        );
        return fallback();
    }
    steps
}

/// Run the configured meeting template (a `scope = Meeting` recipe) once
/// over a meeting's merged transcript, returning `(digest, model)` on success or a
/// human-readable reason on failure (it reaches the UI toast verbatim). This is
/// [`generate_meeting_digest`] generalized: when `cfg.meeting_recipe_id` is empty
/// (the default) it runs the exact built-in digest path; otherwise it runs the
/// named meeting recipe's Digest + Hook steps.
///
/// The Digest step is the recipe's single persisted output — the last Digest
/// step's result is the `(digest, model)` returned and stored against the meeting.
/// Hook steps run with the merged transcript JSON (gated by their keyword trigger,
/// honouring `required` exactly like the per-track path). The empty-merge guard is
/// kept (a meeting with no transcribed track yet is "nothing to digest", not a
/// failure). The `LlmActivity`/skip stream is keyed on `event_id` (the meeting's
/// first track); the result is stored against the meeting by the caller.
pub(crate) async fn run_meeting_recipe(
    state: &AppState,
    cfg: &Config,
    event_id: &RecordingId,
    tracks: &[phoneme_core::Recording],
) -> std::result::Result<(String, String), String> {
    let merged = assemble_meeting_transcript(tracks);
    if merged.trim().is_empty() {
        return Err("no transcribed tracks yet — nothing to digest".into());
    }

    let steps = resolve_meeting_recipe(cfg);
    let mut result: Option<(String, String)> = None;

    for step in steps {
        match step {
            ResolvedMeetingStep::Digest { llm_cfg, prompt } => {
                let endpoint_hint = {
                    let u = llm_cfg.api_url.trim();
                    (!u.is_empty()).then(|| u.to_string())
                };
                let (digest, model) = generate_summary_with(
                    state,
                    event_id,
                    &merged,
                    llm_cfg,
                    &prompt,
                    endpoint_hint.as_deref(),
                )
                .await?;
                result = Some((digest, model));
            }
            ResolvedMeetingStep::Hook { hook } => {
                run_meeting_hook(state, cfg, &hook, &merged, tracks).await;
            }
        }
    }

    result.ok_or_else(|| "the meeting recipe produced no digest step".to_string())
}

/// Run a single meeting-scope Hook step (a `Hook` Playbook entry) once over the
/// merged meeting transcript. Best-effort: a meeting's tracks are already complete,
/// so a side-effect failure is logged and surfaced but never fails the meeting (the
/// per-track `required`-fails-the-recording contract has no terminal-status home at
/// meeting scope, where there is no single recording to quarantine). The payload is
/// built from the meeting's first track (id/timestamp/audio_path) carrying the
/// merged transcript, so a webhook/shell hook sees the whole-meeting text.
async fn run_meeting_hook(
    state: &AppState,
    cfg: &Config,
    hook: &phoneme_core::config::PlaybookHook,
    merged: &str,
    tracks: &[phoneme_core::Recording],
) {
    if !hook.should_run(merged) {
        return;
    }
    let Some(first) = tracks.first() else {
        return;
    };
    let payload = HookPayload {
        id: first.id.clone(),
        timestamp: first.started_at,
        transcript: merged.to_string(),
        audio_path: first.audio_path.clone(),
        duration_ms: first.duration_ms,
        model: first.model.clone().unwrap_or_default(),
        metadata: HookMetadata::current(),
    };
    let timeout = Duration::from_secs(hook.timeout_secs);

    let cmd = hook.command.trim();
    if !cmd.is_empty() {
        let runner = HookRunner::new(phoneme_core::config::expand_cmd(cmd), timeout);
        match runner.run(&payload).await {
            Ok(result) if result.exit_code == 0 => {
                tracing::info!(command = %cmd, "meeting hook ran");
            }
            Ok(result) => {
                tracing::warn!(command = %cmd, exit_code = result.exit_code, "meeting hook exited non-zero");
            }
            Err(e) => {
                tracing::warn!(command = %cmd, error = %e, "meeting hook failed to run");
            }
        }
    }

    let url = hook.webhook_url.trim();
    if !url.is_empty() {
        if let Err(e) = state
            .webhook
            .post(url, timeout, &payload, &cfg.webhook)
            .await
        {
            tracing::warn!(url = %url, error = %e, "meeting webhook failed");
        }
    }
}

/// The instruction a period digest is generated with. A period digest rolls up
/// EVERY recording in a date window into one account, so the transcript handed
/// to the model concatenates many independent recordings (each prefixed with its
/// date + title) rather than the tracks of one meeting. Reuses the `[summary]`
/// provider/model connection (see [`summary_llm_config`]); only the prompt
/// differs, so no new provider keys.
pub(crate) const PERIOD_DIGEST_PROMPT: &str = "You are writing a rollup of everything the user recorded over a period. The transcript below concatenates multiple separate recordings in chronological order, each prefixed with its date/time and title. Produce: a short overview of what was discussed across the period, the key topics, decisions reached, and open/action items (with the owner when stated). Synthesize across recordings; do not summarize each one separately. Output only the digest, with no preamble.";

/// Soft cap on the assembled period transcript, in characters. A period can span
/// a week of recordings whose combined transcripts dwarf the meeting digest's
/// ~2 tracks and can blow past the model's context window (and run up cost). The
/// assembler stops adding recordings once this budget is reached and appends a
/// truncation marker, so a huge window degrades gracefully instead of failing or
/// silently overflowing. ~120k chars is a generous ceiling that still fits a
/// large local context; lowering it trades completeness for cost/latency.
pub(crate) const PERIOD_DIGEST_MAX_CHARS: usize = 120_000;

/// Assemble a single transcript spanning every recording in a date window, for a
/// period digest. Recordings are ordered chronologically (`started_at`, ties
/// broken by id for determinism) and each block is prefixed with its date + title
/// so the model can attribute content to a moment in time. Recordings with no
/// transcript yet (still transcribing, or failed) contribute nothing. Returns an
/// empty string when no recording in the window has any transcript — the caller
/// treats that as "nothing to digest".
///
/// Bounded by [`PERIOD_DIGEST_MAX_CHARS`]: once the running length would exceed
/// the budget, no further recordings are appended and a truncation marker is
/// added, so an enormous window can't overflow the model's context or run up
/// unbounded cost (risk 1 in the design brief). Pure — testable without an LLM.
pub(crate) fn assemble_period_transcript(recordings: &[phoneme_core::Recording]) -> String {
    let mut ordered: Vec<&phoneme_core::Recording> = recordings
        .iter()
        .filter(|r| {
            r.transcript
                .as_deref()
                .map(str::trim)
                .is_some_and(|t| !t.is_empty())
        })
        .collect();
    // The `list` query already sorts, but re-sort defensively (oldest-first) so
    // the merged transcript reads chronologically regardless of caller ordering.
    ordered.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    let mut out = String::new();
    let mut truncated = false;
    for r in ordered {
        let date = r.started_at.format("%Y-%m-%d %H:%M").to_string();
        let title = r
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| r.id.as_str());
        let text = r.transcript.as_deref().unwrap_or("").trim();
        let block = format!("=== {date} — {title} ===\n{text}");
        // Stop before exceeding the budget; account for the "\n\n" joiner. The
        // first block is always included even if it alone exceeds the budget, so
        // a single very long recording still produces (a truncated) digest.
        let added_len = if out.is_empty() {
            block.len()
        } else {
            out.len() + 2 + block.len()
        };
        if !out.is_empty() && added_len > PERIOD_DIGEST_MAX_CHARS {
            truncated = true;
            break;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&block);
        if out.len() >= PERIOD_DIGEST_MAX_CHARS {
            // The block we just added reached/passed the budget; stop here.
            truncated = true;
            break;
        }
    }
    if truncated {
        out.push_str("\n\n=== [transcript truncated: period exceeds the digest size limit; some later recordings were omitted] ===");
    }
    out
}

/// Generate an LLM rollup across every recording in a date window: assemble the
/// window's transcripts into one document ([`assemble_period_transcript`]) and
/// run it through the same summary provider + streaming/persistence path the
/// per-recording summary uses ([`generate_summary_with`]), but with the
/// period-scope [`PERIOD_DIGEST_PROMPT`]. Returns `(digest, model)` on success or
/// a human-readable reason on failure (reaches the UI toast verbatim). The
/// `LlmActivity`/skip events are keyed on `event_id` (the window's first
/// recording), since the activity stream is per-recording; the result is stored
/// against the range, not the recording.
pub(crate) async fn generate_period_digest(
    state: &AppState,
    cfg: &Config,
    event_id: &RecordingId,
    recordings: &[phoneme_core::Recording],
) -> std::result::Result<(String, String), String> {
    let merged = assemble_period_transcript(recordings);
    if merged.trim().is_empty() {
        return Err("no transcribed recordings in that range — nothing to digest".into());
    }
    let endpoint_hint = cfg.summary.api_url.trim();
    let endpoint_hint = (!endpoint_hint.is_empty()).then(|| endpoint_hint.to_string());
    // Reuse the summary connection (provider/model/key/url) but the period-scope
    // prompt. `generate_summary_with` gives identical events + skip/empty/error
    // classification + recorded model as the per-recording summary.
    generate_summary_with(
        state,
        event_id,
        &merged,
        summary_llm_config(cfg),
        PERIOD_DIGEST_PROMPT,
        endpoint_hint.as_deref(),
    )
    .await
}
