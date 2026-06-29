//! Structured-extraction pipeline steps: entities, chapters, and tasks
//! (their LLM-config builders, parsers, on-demand extract entry points, and
//! per-step runners). Split out of pipeline.rs; `use super::*` pulls in the
//! shared helpers (run_llm_stage, finalize_step_status, entry_config_for_target).

use super::*;

/// The built-in instruction the entity-extraction step uses when the recording
/// has no migrated `entities` Playbook entry (a user deleted it) — the entity
/// counterpart of `[auto_tag].prompt`. Asks for a small JSON array of
/// `{kind,value}` so [`parse_entities`] can scan it robustly. Kept in code (not
/// a config section) because there is no `[entities]` section today; the Playbook
/// `entities` entry is the editable source of truth.
pub(crate) const DEFAULT_ENTITY_PROMPT: &str = "Extract the key named entities from this transcript. Reply with ONLY a JSON array of objects, each {\"kind\":\"...\",\"value\":\"...\"}, where kind is one of: person, org, topic, term. Use \"person\" for people, \"org\" for organizations/companies, \"topic\" for subjects discussed, \"term\" for notable jargon or proper terms. Output at most 20 entities, no duplicates, no preamble, no code fences.";

/// The valid entity kinds. An LLM-emitted kind that isn't one of these is
/// normalized to `topic` by [`parse_entities`] — the most general bucket — so a
/// stray class never drops the entity.
const ENTITY_KINDS: [&str; 4] = ["person", "org", "topic", "term"];

/// Hard cap on how many entities one extraction run stores, mirroring the
/// auto-tag list cap. Keeps a chatty model from flooding the child table.
const MAX_ENTITIES: usize = 20;

/// Build the effective LLM config for entity extraction, mirroring
/// `auto_tag_llm_config`: resolve the migrated `entities` Playbook entry's LLM
/// half against `[llm_post_process]` (inherit-on-blank). Falls back to the bare
/// cleanup connection when no `entities` entry exists, so the on-demand path
/// always has a usable provider.
pub fn entities_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    match entry_config_for_target(cfg, "entities") {
        Some((llm_cfg, _)) => llm_cfg,
        None => cfg.llm_post_process.clone(),
    }
}

/// One parsed `{kind, value}` shape from the model's JSON reply, before
/// normalization. `kind` is optional so a model that emits a bare `{"value":…}`
/// still parses (it defaults to `topic`).
#[derive(serde::Deserialize)]
struct RawEntity {
    #[serde(default)]
    kind: Option<String>,
    value: String,
}

/// Parse the entity-extractor's reply into clean, typed [`Entity`] values.
///
/// Mirrors `parse_tag_names`' robustness: scan every `[` and take the first
/// position that deserializes as a JSON array of `{kind,value}` objects, so
/// chatty models that wrap the array in bracket-bearing prose don't poison it.
/// Each entry is trimmed; an empty or over-long value is dropped; the `kind` is
/// lowercased and normalized to one of [`ENTITY_KINDS`] (an unknown kind →
/// `topic`); case-insensitive `(kind, value)` duplicates collapse; the list is
/// capped at `max`. Returns an empty vec when nothing usable parses (no JSON
/// array, all-empty values) — the caller treats that as "nothing extracted".
pub(crate) fn parse_entities(raw: &str, max: usize) -> Vec<Entity> {
    let cleaned = raw.trim();
    // Find the first JSON array anywhere in the reply that yields at least one
    // well-formed entity, scanning each '[' (same rationale as parse_tag_names):
    // a greedy first-'['..last-']' slice would span bracket-bearing prose and
    // fail to parse. Each candidate '[' is parsed as untyped `Value`s and then
    // each element is deserialized into `RawEntity` *separately*, so a single
    // malformed object (a missing/null/non-string `value`) can't reject the whole
    // batch — the bad element is skipped and every valid sibling is kept. The
    // per-position "at least one valid entity" gate keeps a stray non-entity array
    // earlier in the prose (e.g. `[1, 2]`) from being mistaken for the answer.
    let candidates: Vec<RawEntity> = cleaned
        .char_indices()
        .filter(|(_, c)| *c == '[')
        .find_map(|(start, _)| {
            let elems = serde_json::Deserializer::from_str(&cleaned[start..])
                .into_iter::<Vec<serde_json::Value>>()
                .next()?
                .ok()?;
            let parsed: Vec<RawEntity> = elems
                .into_iter()
                .filter_map(|v| serde_json::from_value::<RawEntity>(v).ok())
                .collect();
            if parsed.is_empty() {
                None
            } else {
                Some(parsed)
            }
        })
        .unwrap_or_default();
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut out: Vec<Entity> = Vec::new();
    for c in candidates {
        let value = c
            .value
            .trim()
            .trim_matches(|ch| ch == '"' || ch == '\'' || ch == '`')
            .trim()
            .to_string();
        // Entity values are short surface strings; anything sentence-length is the
        // model ignoring instructions — drop it rather than storing junk.
        if value.is_empty() || value.chars().count() > 80 {
            continue;
        }
        // Normalize the kind to a known bucket; an unknown/blank kind → "topic".
        let kind_lc = c.kind.unwrap_or_default().trim().to_lowercase();
        let kind = if ENTITY_KINDS.contains(&kind_lc.as_str()) {
            kind_lc
        } else {
            "topic".to_string()
        };
        let key = (kind.clone(), value.to_lowercase());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(Entity { kind, value });
        if out.len() >= max {
            break;
        }
    }
    out
}

/// Extract structured entities for `transcript` and persist them on the recording
/// (replacing any previous set), emitting `EntitiesUpdated` so the UI shows the
/// typed chips. The entity counterpart of [`suggest_tags`]: the legacy/IPC path
/// reads the migrated `entities` Playbook entry (or the built-in default prompt
/// when absent). Non-fatal: failures are logged and surfaced. Returns
/// `Some(error)` only when the step actually failed (an LLM call error) — the
/// caller folds that into the terminal status. An empty transcript, a missing
/// provider, a user-skip, or "nothing extracted" are all non-failures (`None`).
pub async fn extract_entities(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
) -> Option<String> {
    let (llm_cfg, prompt) = match entry_config_for_target(cfg, "entities") {
        Some(pair) => pair,
        None => (entities_llm_config(cfg), DEFAULT_ENTITY_PROMPT.to_string()),
    };
    // On-demand: fall back to the global LLM when the `entities` entry is "none".
    let llm_cfg = ondemand_connection(&state.llm, cfg, llm_cfg);
    extract_entities_with(state, id, transcript, llm_cfg, &prompt).await
}

/// The entity-extractor's core, parameterized by an already-resolved LLM config +
/// prompt so the legacy/IPC path ([`extract_entities`]) and the recipe executor
/// (reads the `entities` entry) share one implementation — same provider mint,
/// streaming, parse, persistence, events, and skip/empty/error classification as
/// the auto-tag path it is modelled on. Records the model via `set_entities_model`
/// once per run, only after a non-empty parse, so the model column never advances
/// past the entities actually stored — on an empty parse the prior entities (and
/// their model) are kept untouched.
pub(crate) async fn extract_entities_with(
    state: &AppState,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    if transcript.trim().is_empty() {
        return None;
    }
    let llm = match llm_provider_for_run(state, &llm_cfg).await {
        Some(llm) => llm,
        None => {
            tracing::warn!(
                provider = %llm_cfg.provider,
                "entity extraction requested but no usable LLM provider is configured"
            );
            return None;
        }
    };
    match run_llm_stage(
        state,
        id,
        // No dedicated `Extracting` stage exists (see the IPC `PipelineStage`
        // note) — reuse Tagging for the live activity stream; the dedicated
        // EntitiesUpdated/Failed events carry the structured result.
        PipelineStage::Tagging,
        &*llm,
        prompt,
        transcript,
    )
    .await
    {
        Ok(reply) => {
            let entities = parse_entities(&reply, MAX_ENTITIES);
            if entities.is_empty() {
                // Keep the prior entities (and their model) on an empty parse —
                // the same deliberate keep-prior-on-empty behavior the auto-tag
                // path uses. The model column is written only below, *after* this
                // guard, so it never advances past the entities actually stored:
                // recording it here would name a model that produced nothing while
                // the displayed entities came from an earlier run. Still emit
                // EntitiesUpdated so an on-demand Extract resolves rather than
                // looking like it silently did nothing — "ran, found nothing" is
                // now distinguishable from a crash (which fires EntitiesFailed).
                tracing::info!(id = %id.as_str(), "entity extraction produced nothing");
                state
                    .events
                    .emit(DaemonEvent::EntitiesUpdated { id: id.clone() });
                return None;
            }
            match state.catalog.set_entities(id, &entities).await {
                Ok(()) => {
                    // Record which model ran the extractor (the detail provenance
                    // line names it), only once the entities are stored — so the
                    // model column never advances past the entities actually saved
                    // (matches the tasks path). A failed store leaves the prior
                    // entities and their model untouched.
                    if let Err(e) = state.catalog.set_entities_model(id, &llm_cfg.model).await {
                        tracing::warn!(error = %e, "failed to persist entities model");
                    }
                    tracing::info!(id = %id.as_str(), count = entities.len(), "entities saved");
                    state
                        .events
                        .emit(DaemonEvent::EntitiesUpdated { id: id.clone() });
                }
                Err(e) => tracing::warn!(error = %e, "failed to persist entities"),
            }
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "entity extraction LLM call failed");
            // Best-effort: no entities added; surface the failure for a toast +
            // the terminal status. A user-skip carries the sentinel and isn't a
            // failure.
            let skipped = stage_skipped(&e);
            let msg = e.to_string();
            state.events.emit(DaemonEvent::EntitiesFailed {
                id: id.clone(),
                error: msg.clone(),
            });
            if skipped {
                None
            } else {
                Some(msg)
            }
        }
    }
}

/// Run the entity-extraction step from the recipe executor: emit the
/// `PipelineStageChanged(Tagging)` the UI reads, then run the shared
/// [`extract_entities_with`] core. Mirrors [`run_tags_step`].
pub(crate) async fn run_entities_step(
    state: &AppState,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Tagging,
    });
    extract_entities_with(state, id, transcript, llm_cfg, prompt).await
}

// ── Auto-chapters ──────────────────────────────────────────────────────────────
// The auto-chapter enrichment is the entity step's twin, with one twist: a chapter
// is a *time range*, so the model must be anchored to the recording's real segment
// start times rather than emitting free-form milliseconds (it would hallucinate
// them). The step feeds the model a numbered segment list with `start_ms` anchors,
// asks it to pick boundaries from those anchors, then `parse_chapters` snaps every
// returned start to the nearest real segment start, sorts, derives each `end_ms`
// from the next start (the last ends at the recording's `duration_ms`), and drops
// overlaps. Reuses the Tagging stage for the live stream and folds a real failure
// into `TagFailed`, exactly like entities.

/// The built-in instruction the auto-chapter step uses when the recording has no
/// `chapters` Playbook entry (a user deleted it). Asks for a JSON array of
/// `{start_ms,title,summary}` where each `start_ms` is one of the supplied segment
/// start times — never a free-form value — so [`parse_chapters`] can anchor the
/// boundaries to the audio. Kept in code (not a config section) because there is no
/// `[chapters]` section; the Playbook `chapters` entry is the editable source of
/// truth (mirrors [`DEFAULT_ENTITY_PROMPT`]).
pub(crate) const DEFAULT_CHAPTERS_PROMPT: &str = "Divide this transcript into topic chapters. You are given the transcript as a numbered list of segments, each prefixed with its start time in milliseconds in [brackets]. Reply with ONLY a JSON array of objects, each {\"start_ms\":<one of the bracketed segment start times>,\"title\":\"short topic label\",\"summary\":\"one-line description\"}, in chronological order. Choose boundaries where the topic genuinely shifts; start the first chapter at the earliest segment. The start_ms MUST be copied exactly from one of the bracketed start times — never invent a millisecond value. Output at most 20 chapters, no preamble, no code fences.";

/// Hard cap on how many chapters one run stores, mirroring [`MAX_ENTITIES`]. Keeps
/// a chatty model from flooding the child table and the timeline view.
const MAX_CHAPTERS: usize = 20;

/// Build the effective LLM config for the auto-chapter step, mirroring
/// [`entities_llm_config`]: resolve the `chapters` Playbook entry's LLM half
/// against `[llm_post_process]` (inherit-on-blank), falling back to the bare
/// cleanup connection when no `chapters` entry exists, so the on-demand path always
/// has a usable provider.
pub fn chapters_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    match entry_config_for_target(cfg, "chapters") {
        Some((llm_cfg, _)) => llm_cfg,
        None => cfg.llm_post_process.clone(),
    }
}

// ── Tasks / action items ─────────────────────────────────────────────────────────

/// The built-in instruction the task-extraction step uses when the recording has
/// no migrated `tasks` Playbook entry (a user deleted it) — the task counterpart
/// of [`DEFAULT_ENTITY_PROMPT`]. Asks for a small JSON array of `{text,due}` so
/// [`parse_tasks`] can scan it robustly. Kept in code (not a config section)
/// because there is no `[tasks]` section today; the Playbook `tasks` entry is the
/// editable source of truth.
pub(crate) const DEFAULT_TASK_PROMPT: &str = "Extract concrete action items or to-dos the speaker committed to or asked for. Reply with ONLY a JSON array of objects, each {\"text\":\"...\",\"due\":\"...\"}, where text is the action (imperative and short) and due is any deadline mentioned (a free-text phrase like \"by Friday\", or an empty string if none). Output at most 20 items, no duplicates, no preamble, no code fences.";

/// Hard cap on how many tasks one extraction run stores, mirroring
/// [`MAX_ENTITIES`]. Keeps a chatty model from flooding the child table.
const MAX_TASKS: usize = 20;

/// Build the effective LLM config for task extraction, mirroring
/// [`entities_llm_config`]: resolve the migrated `tasks` Playbook entry's LLM
/// half against `[llm_post_process]` (inherit-on-blank). Falls back to the bare
/// cleanup connection when no `tasks` entry exists, so the on-demand path always
/// has a usable provider.
pub fn tasks_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    match entry_config_for_target(cfg, "tasks") {
        Some((llm_cfg, _)) => llm_cfg,
        None => cfg.llm_post_process.clone(),
    }
}

/// One parsed `{start_ms, title, summary}` shape from the model's JSON reply,
/// before snapping/validation. Every field is optional except a title-bearing
/// entry needs a `title`; a missing `start_ms` drops the entry (it can't be
/// anchored).
#[derive(serde::Deserialize)]
struct RawChapter {
    start_ms: Option<i64>,
    title: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// Build the model input: a numbered segment list, each line prefixed with its
/// `start_ms` anchor in `[brackets]`, so the model can only choose boundaries from
/// real segment starts. Mirrors how the timeline view groups segments, but flat
/// and timestamped for the LLM.
fn chapters_prompt_input(segments: &[TranscriptSegment]) -> String {
    let mut buf = String::new();
    for (i, s) in segments.iter().enumerate() {
        // One line per segment: index, the bracketed start_ms anchor, the text.
        buf.push_str(&format!("{}. [{}] {}\n", i + 1, s.start_ms, s.text.trim()));
    }
    buf
}

/// Snap a model-returned `start_ms` to the nearest real segment start. The segment
/// starts are sorted ascending (segments come from the catalog in `idx`/timeline
/// order); a binary search finds the insertion point and the closer of the two
/// neighbours wins. `starts` is assumed non-empty (the caller short-circuits an
/// empty segment set before parsing).
fn snap_to_nearest_start(target: i64, starts: &[i64]) -> i64 {
    match starts.binary_search(&target) {
        Ok(i) => starts[i],
        Err(i) => {
            if i == 0 {
                starts[0]
            } else if i >= starts.len() {
                starts[starts.len() - 1]
            } else {
                let lo = starts[i - 1];
                let hi = starts[i];
                if target - lo <= hi - target {
                    lo
                } else {
                    hi
                }
            }
        }
    }
}

/// Parse the auto-chapter reply into anchored, validated [`Chapter`] ranges — the
/// load-bearing correctness step.
///
/// `segments` is the recording's real timeline (already in start order); the model
/// is never trusted to emit timing. The flow:
/// 1. Scan every `[` for the first JSON array of `{start_ms,title,...}` objects
///    that yields at least one valid chapter (the robust scan
///    [`parse_entities`] uses, so bracket-bearing prose doesn't poison it). Each
///    element is deserialized separately, so one malformed object never rejects the
///    batch.
/// 2. Drop entries with a blank title or a missing `start_ms` (it can't be
///    anchored).
/// 3. **Snap** each `start_ms` to the nearest real segment start, so a boundary
///    always lands on the audio even if the model copied a value imperfectly.
/// 4. Sort by the snapped start and **de-duplicate** snapped starts (two model
///    boundaries snapping to the same segment collapse to one — keep the first,
///    so out-of-order/overlapping inputs can't produce a zero/negative-width
///    chapter).
/// 5. Derive each `end_ms` from the next chapter's start; the last ends at
///    `duration_ms` (clamped so it's never before its own start).
/// 6. Cap at `max`.
///
/// Returns an empty vec when nothing usable parses (no JSON array, all-blank
/// titles, all starts past the audio) — the caller keeps any prior chapters on an
/// empty parse, exactly like entities.
pub(crate) fn parse_chapters(
    raw: &str,
    segments: &[TranscriptSegment],
    duration_ms: i64,
    max: usize,
) -> Vec<Chapter> {
    if segments.is_empty() {
        return Vec::new();
    }
    let starts: Vec<i64> = segments.iter().map(|s| s.start_ms).collect();
    let cleaned = raw.trim();
    let candidates: Vec<RawChapter> = cleaned
        .char_indices()
        .filter(|(_, c)| *c == '[')
        .find_map(|(start, _)| {
            let elems = serde_json::Deserializer::from_str(&cleaned[start..])
                .into_iter::<Vec<serde_json::Value>>()
                .next()?
                .ok()?;
            let parsed: Vec<RawChapter> = elems
                .into_iter()
                .filter_map(|v| serde_json::from_value::<RawChapter>(v).ok())
                .collect();
            // A valid candidate array has at least one entry carrying both a
            // start_ms and a non-blank title — otherwise it's a stray earlier array
            // (e.g. `[1, 2]`), keep scanning.
            if parsed.iter().any(|c| {
                c.start_ms.is_some() && c.title.as_ref().is_some_and(|t| !t.trim().is_empty())
            }) {
                Some(parsed)
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Snap + collect (start, title, summary), dropping un-anchorable / untitled.
    let mut snapped: Vec<(i64, String, Option<String>)> = Vec::new();
    for c in candidates {
        let Some(raw_start) = c.start_ms else {
            continue;
        };
        let title = c.title.unwrap_or_default().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let start = snap_to_nearest_start(raw_start, &starts);
        let summary = c
            .summary
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        snapped.push((start, title, summary));
    }
    // Sort by snapped start; de-dup colliding starts (keep the first, so a stable
    // chapter survives and no zero-width range is produced).
    snapped.sort_by_key(|(start, _, _)| *start);
    snapped.dedup_by_key(|(start, _, _)| *start);
    snapped.truncate(max);

    // Derive each end_ms from the next chapter's snapped start; the last chapter
    // ends at the recording duration. Both are clamped so an end is never before its
    // own start (the sort + dedup already guarantee strictly increasing starts, but
    // a duration shorter than the last start — a truncated/odd recording — would
    // otherwise produce a backwards last range).
    let n = snapped.len();
    let mut out: Vec<Chapter> = Vec::with_capacity(n);
    for i in 0..n {
        let (start, ref title, ref summary) = snapped[i];
        let end = if i + 1 < n {
            snapped[i + 1].0.max(start)
        } else {
            duration_ms.max(start)
        };
        out.push(Chapter {
            start_ms: start,
            end_ms: end,
            title: title.clone(),
            summary: summary.clone(),
        });
    }
    out
}

/// One parsed `{text, due}` shape from the model's JSON reply, before
/// normalization. `due` is optional so a model that emits a bare `{"text":…}`
/// still parses (it defaults to no due hint).
#[derive(serde::Deserialize)]
struct RawTask {
    text: String,
    #[serde(default)]
    due: Option<String>,
}

/// Parse the task-extractor's reply into clean [`Task`] values.
///
/// Mirrors [`parse_entities`]' robustness: scan every `[` and take the first
/// position that deserializes as a JSON array of `{text,due}` objects, so chatty
/// models that wrap the array in bracket-bearing prose don't poison it. Each
/// entry's `text` is trimmed; an empty or over-long text is dropped; `due` is
/// trimmed (a blank `due` becomes `None`); case-insensitive duplicate texts
/// collapse; the list is capped at `max`. A freshly-extracted task is never
/// pre-checked (`done = false`); the `id` is a placeholder (`0`) until the row is
/// stored — [`Catalog::set_tasks`] assigns the real id. Returns an empty vec when
/// nothing usable parses — the caller treats that as "nothing extracted".
pub(crate) fn parse_tasks(raw: &str, max: usize) -> Vec<Task> {
    let cleaned = raw.trim();
    // Same per-`[` scan + per-element deserialize as parse_entities: a single
    // malformed object can't reject the whole batch, and a stray non-task array
    // earlier in the prose (e.g. `[1, 2]`) is skipped by the "at least one valid
    // task" gate.
    let candidates: Vec<RawTask> = cleaned
        .char_indices()
        .filter(|(_, c)| *c == '[')
        .find_map(|(start, _)| {
            let elems = serde_json::Deserializer::from_str(&cleaned[start..])
                .into_iter::<Vec<serde_json::Value>>()
                .next()?
                .ok()?;
            let parsed: Vec<RawTask> = elems
                .into_iter()
                .filter_map(|v| serde_json::from_value::<RawTask>(v).ok())
                .collect();
            if parsed.is_empty() {
                None
            } else {
                Some(parsed)
            }
        })
        .unwrap_or_default();
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<Task> = Vec::new();
    for c in candidates {
        let text = c
            .text
            .trim()
            .trim_matches(|ch| ch == '"' || ch == '\'' || ch == '`')
            .trim()
            .to_string();
        // Action items are short imperative phrases; a paragraph is the model
        // ignoring instructions — drop it rather than store junk.
        if text.is_empty() || text.chars().count() > 200 {
            continue;
        }
        let key = text.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let due_hint = c.due.and_then(|d| {
            let t = d.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        });
        out.push(Task {
            id: 0,
            text,
            due_hint,
            done: false,
        });
        if out.len() >= max {
            break;
        }
    }
    out
}

/// Extract auto-chapters for `id` and persist them (replacing any previous set),
/// emitting `ChaptersUpdated` so the UI shows the new chapter rows. The legacy/IPC
/// path reads the `chapters` Playbook entry (or the built-in default prompt when
/// absent). Non-fatal like [`extract_entities`]: returns `Some(error)` only when
/// the step actually failed (an LLM call error) — an empty transcript, no segments
/// (no timing to chapter), a missing provider, a user-skip, or "nothing parsed"
/// are all clean non-failures (`None`).
pub async fn extract_chapters(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
) -> Option<String> {
    let (llm_cfg, prompt) = match entry_config_for_target(cfg, "chapters") {
        Some(pair) => pair,
        None => (
            chapters_llm_config(cfg),
            DEFAULT_CHAPTERS_PROMPT.to_string(),
        ),
    };
    // On-demand: fall back to the global LLM when the `chapters` entry is "none".
    let llm_cfg = ondemand_connection(&state.llm, cfg, llm_cfg);
    extract_chapters_with(state, id, transcript, llm_cfg, &prompt).await
}

/// The auto-chapter core, parameterized by a resolved LLM config + prompt so the
/// legacy/IPC path ([`extract_chapters`]) and the recipe executor share one
/// implementation — same provider mint, streaming, parse, persistence, events, and
/// skip/empty/error classification as the entity path it mirrors.
///
/// Unlike entities, this loads the recording's segments (timing) fresh from the
/// catalog: chapters anchor to segment starts, and the executor passes around the
/// *prose* transcript, not segments — which are persisted earlier in the transcribe
/// phase, so they're available by the time enrichments run. A recording with no
/// segments can't be chaptered: short-circuit to a clean non-failure (`None`),
/// like [`extract_entities_with`] does on an empty transcript. The model is fed the
/// numbered segment list (with `start_ms` anchors), not the raw `transcript` — but
/// an empty `transcript` still short-circuits first, matching the entity path.
pub(crate) async fn extract_chapters_with(
    state: &AppState,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    if transcript.trim().is_empty() {
        return None;
    }
    // Chapters need the recording's real segment timing + duration; both come from
    // the catalog (segments land in the transcribe phase, before enrichments run).
    let segments = match state.catalog.segments_for(id).await {
        Ok(segs) => segs,
        Err(e) => {
            tracing::warn!(error = %e, "auto-chapter: failed to load segments");
            return None;
        }
    };
    if segments.is_empty() {
        // No timing to chapter — a normal non-failure (the recording predates
        // segment capture, or its provider returned none). Mirrors the empty-
        // transcript short-circuit; the view shows a "re-run Transcribe" hint.
        tracing::info!(id = %id.as_str(), "auto-chapter: no segments, nothing to chapter");
        return None;
    }
    let duration_ms = match state.catalog.get(id).await {
        Ok(Some(rec)) => rec.duration_ms,
        // No row (deleted mid-run) or a read error: fall back to the last segment's
        // end so the final chapter still has a sane end_ms rather than failing.
        _ => segments.last().map(|s| s.end_ms).unwrap_or(0),
    };
    let llm = match llm_provider_for_run(state, &llm_cfg).await {
        Some(llm) => llm,
        None => {
            tracing::warn!(
                provider = %llm_cfg.provider,
                "auto-chapter requested but no usable LLM provider is configured"
            );
            return None;
        }
    };
    let model_input = chapters_prompt_input(&segments);
    match run_llm_stage(
        state,
        id,
        // No dedicated chapters stage — reuse Tagging for the live activity stream,
        // exactly as entities does; the ChaptersUpdated/Failed events carry the
        // structured result.
        PipelineStage::Tagging,
        &*llm,
        prompt,
        &model_input,
    )
    .await
    {
        Ok(reply) => {
            let chapters = parse_chapters(&reply, &segments, duration_ms, MAX_CHAPTERS);
            if chapters.is_empty() {
                // Keep any prior chapters (and their model) on an empty parse — the
                // same keep-prior-on-empty behavior the entity path uses. The model
                // column is written only below, after this guard, so it never
                // advances past the chapters actually stored.
                tracing::info!(id = %id.as_str(), "auto-chapter produced nothing");
                return None;
            }
            match state.catalog.replace_chapters(id, &chapters).await {
                Ok(()) => {
                    if let Err(e) = state.catalog.set_chapters_model(id, &llm_cfg.model).await {
                        tracing::warn!(error = %e, "failed to persist chapters model");
                    }
                    tracing::info!(id = %id.as_str(), count = chapters.len(), "chapters saved");
                    state
                        .events
                        .emit(DaemonEvent::ChaptersUpdated { id: id.clone() });
                }
                Err(e) => tracing::warn!(error = %e, "failed to persist chapters"),
            }
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "auto-chapter LLM call failed");
            let skipped = stage_skipped(&e);
            let msg = e.to_string();
            state.events.emit(DaemonEvent::ChaptersFailed {
                id: id.clone(),
                error: msg.clone(),
            });
            if skipped {
                None
            } else {
                Some(msg)
            }
        }
    }
}

/// Run the auto-chapter step from the recipe executor: emit the
/// `PipelineStageChanged(Tagging)` the UI reads, then run the shared
/// [`extract_chapters_with`] core. Mirrors [`run_entities_step`].
pub(crate) async fn run_chapters_step(
    state: &AppState,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Tagging,
    });
    extract_chapters_with(state, id, transcript, llm_cfg, prompt).await
}

/// Extract task / action items for `transcript` and persist them on the recording
/// (done-preserving, via [`Catalog::set_tasks`]), emitting `TasksUpdated` so the
/// UI shows the togglable task chips. The task counterpart of
/// [`extract_entities`]: the legacy/IPC path reads the migrated `tasks` Playbook
/// entry (or the built-in default prompt when absent). Non-fatal; returns
/// `Some(error)` only on a real LLM-call failure (the caller folds it into the
/// terminal status). Empty transcript / missing provider / user-skip / nothing
/// extracted are all non-failures (`None`).
pub async fn extract_tasks(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
) -> Option<String> {
    let (llm_cfg, prompt) = match entry_config_for_target(cfg, "tasks") {
        Some(pair) => pair,
        None => (tasks_llm_config(cfg), DEFAULT_TASK_PROMPT.to_string()),
    };
    // On-demand: fall back to the global LLM when the `tasks` entry is "none".
    let llm_cfg = ondemand_connection(&state.llm, cfg, llm_cfg);
    extract_tasks_with(state, id, transcript, llm_cfg, &prompt).await
}

/// The task-extractor's core, parameterized by an already-resolved LLM config +
/// prompt so the legacy/IPC path ([`extract_tasks`]) and the recipe executor
/// share one implementation. Reuses [`PipelineStage::Tagging`] for the live
/// activity stream (entities do too — no dedicated stage exists). Records the
/// model via `set_tasks_model` once per run, only after a non-empty parse, so the
/// model column never advances past the tasks actually stored — on an empty parse
/// the prior tasks (and their model) are kept untouched. [`Catalog::set_tasks`]
/// preserves any `done` flag the user set on a surviving task.
pub(crate) async fn extract_tasks_with(
    state: &AppState,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    if transcript.trim().is_empty() {
        return None;
    }
    let llm = match llm_provider_for_run(state, &llm_cfg).await {
        Some(llm) => llm,
        None => {
            tracing::warn!(
                provider = %llm_cfg.provider,
                "task extraction requested but no usable LLM provider is configured"
            );
            return None;
        }
    };
    match run_llm_stage(
        state,
        id,
        // Reuse Tagging for the live activity stream (like entities); the
        // dedicated TasksUpdated/Failed events carry the structured result.
        PipelineStage::Tagging,
        &*llm,
        prompt,
        transcript,
    )
    .await
    {
        Ok(reply) => {
            let tasks = parse_tasks(&reply, MAX_TASKS);
            if tasks.is_empty() {
                // Keep the prior tasks (and their model) on an empty parse — the
                // same keep-prior-on-empty behavior the entity path uses, so a
                // flaky model run never erases the user's task list. The model
                // column is written only below, after this guard. Still emit
                // TasksUpdated so an on-demand Extract resolves (the detail view
                // refreshes / the button's pending state clears) instead of
                // looking like it silently did nothing — "ran, found nothing" is
                // now distinguishable from a crash (which fires TasksFailed).
                tracing::info!(id = %id.as_str(), "task extraction produced nothing");
                state
                    .events
                    .emit(DaemonEvent::TasksUpdated { id: id.clone() });
                return None;
            }
            match state.catalog.set_tasks(id, &tasks).await {
                Ok(()) => {
                    // Record which model ran, only once tasks are stored, so the
                    // model column never disagrees with the stored set.
                    if let Err(e) = state.catalog.set_tasks_model(id, &llm_cfg.model).await {
                        tracing::warn!(error = %e, "failed to persist tasks model");
                    }
                    tracing::info!(id = %id.as_str(), count = tasks.len(), "tasks saved");
                    state
                        .events
                        .emit(DaemonEvent::TasksUpdated { id: id.clone() });
                }
                Err(e) => tracing::warn!(error = %e, "failed to persist tasks"),
            }
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "task extraction LLM call failed");
            let skipped = stage_skipped(&e);
            let msg = e.to_string();
            state.events.emit(DaemonEvent::TasksFailed {
                id: id.clone(),
                error: msg.clone(),
            });
            if skipped {
                None
            } else {
                Some(msg)
            }
        }
    }
}

/// Run the task-extraction step from the recipe executor: emit the
/// `PipelineStageChanged(Tagging)` the UI reads, then run the shared
/// [`extract_tasks_with`] core. Mirrors [`run_entities_step`].
pub(crate) async fn run_tasks_step(
    state: &AppState,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Tagging,
    });
    extract_tasks_with(state, id, transcript, llm_cfg, prompt).await
}
