/**
 * This module provides the frontend TypeScript boundary to the Tauri Rust backend.
 * It encapsulates the `invoke` calls into strictly typed async functions.
 *
 * Path of a call: a function here → Tauri `invoke("<command>")` → the tray's
 * `#[tauri::command]` in src-tauri/src/commands.rs → BridgeSlot (the tray's
 * pipe connection to the daemon) → the daemon's ipc_handler. The command
 * names and payload shapes therefore mirror commands.rs, which in turn
 * mirrors the wire schema in crates/phoneme-ipc/src/schema.rs.
 *
 * Error behavior: every function rejects on failure with the structured
 * `{ kind, message }` command error (see utils/error.ts). Nothing here
 * toasts — callers decide how to surface failures.
 *
 * House rules for adding a call: argument keys are camelCase (Tauri converts
 * the top-level keys to the command's snake_case parameters, but not the keys
 * nested inside object values, so wire-shaped objects like `ListFilter` and
 * `RerunAllOverrides` keep snake_case fields), and mutations that change
 * catalog state come back to the UI as daemon events, not return values.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";

/**
 * One catalog row, as the daemon serializes it (snake_case fields). The core
 * identity/audio fields are always present; most enrichment fields are
 * optional because older rows predate their features. `status` holds the
 * pipeline state ("recording", "transcribing", …, "done", "transcribe_failed",
 * "hook_failed", "cancelled" — see utils/format.ts `statusLabel`).
 */
export type Recording = {
  id: string;
  started_at: string;
  duration_ms: number;
  audio_path: string;
  transcript: string | null;
  model: string | null;
  status: string;
  error_kind?: string | null;
  error_message?: string | null;
  hook_command?: string | null;
  hook_exit_code?: number | null;
  hook_duration_ms?: number | null;
  transcribed_at?: string | null;
  hook_ran_at?: string | null;
  /** Free-form user notes, stored separately from the transcript. */
  notes?: string | null;
  /** Meeting-session link (v1.6). Two recordings of one meeting share this. */
  meeting_id?: string | null;
  /** Which track of a meeting this is: "mic" or "system". Null otherwise. */
  track?: string | null;
  meeting_name?: string | null;
  /** LLM model used for post-processing cleanup */
  cleanup_model?: string | null;
  /** Whether speaker diarization was applied */
  diarized?: boolean;
  /** The model/engine that produced the diarization, when the daemon recorded
   *  it per-recording (e.g. a cloud diarizer). Null for the built-in local
   *  diarizer or older rows; the provenance line falls back to "diarized". */
  diarization_model?: string | null;
  /** Whether this recording was captured via in-place dictation (typed into the
   *  focused window) rather than the normal record→transcribe flow. */
  in_place?: boolean;
  /** Whether the user hand-edited the transcript (independent of `model`). */
  user_edited?: boolean;
  /** Whether the user has starred this recording (the Favorites view). */
  favorite?: boolean;
  /** Whether the user has pinned this recording. Pinned recordings sort to the
   *  top of the library, independent of `favorite`. */
  pinned?: boolean;
  /** LLM-suggested tags awaiting approval (auto-tagging). Names only. */
  tag_suggestions?: string[];
  /** LLM-generated summary of the transcript, if one has been produced. */
  summary?: string | null;
  /** The LLM model used to produce `summary`, if any. */
  summary_model?: string | null;
  /** Structured, typed entities (person/org/topic/term) extracted from the
   *  transcript by the entity-extraction step — richer than the flat
   *  `tag_suggestions`. Empty until extracted. */
  entities?: Entity[];
  /** The LLM model used by the entity-extraction step, when recorded. Null for
   *  older rows or recordings whose entities were never extracted. */
  entities_model?: string | null;
  /** Task / action items extracted from the transcript by the task-extraction
   *  step. Open tasks sort first. Empty until extracted. */
  tasks?: Task[];
  /** The LLM model used by the task-extraction step, when recorded. Null for
   *  older rows or recordings whose tasks were never extracted. */
  tasks_model?: string | null;
  /** Display title — auto-generated (heuristic/LLM) or user-set. Null until
   *  generated; the UI falls back to the started-at timestamp. */
  title?: string | null;
  /** Whether `title` is auto-generated (true — the pipeline may refresh it)
   *  or user-set (false — auto writes never overwrite it). */
  title_is_auto?: boolean;
  /** The LLM model used to auto-generate `title`, when the daemon recorded it
   *  per-recording. Null for a heuristic title or older rows; the provenance
   *  line falls back to "auto-title". */
  title_model?: string | null;
  /** The LLM model used by the auto-tagger for this recording, when recorded.
   *  Null for older rows or recordings that weren't auto-tagged; the provenance
   *  line falls back to inferring the step from pending `tag_suggestions`. */
  tag_model?: string | null;
  /** Mean per-word ASR confidence in 0..1, computed when transcription completed
   *  (the signal behind the low-confidence badge + filter). Null for recordings
   *  transcribed before this existed, for providers that return no per-word
   *  confidence (the OpenAI/Groq cloud transcription endpoints emit none), and
   *  for empty transcripts — a null aggregate shows no badge and is never
   *  flagged, so older rows and cloud transcripts degrade silently. */
  mean_confidence?: number | null;
  /** The spoken language the transcriber detected for this recording (a BCP-47/
   *  ISO-639 code, e.g. "en", "es"). Drives the detail-pane "detected" badge and
   *  the spoken-language router. Null for older rows and providers/paths that
   *  surface no language (the native path, the gpt-4o-transcribe family, plain
   *  non-verbose responses); a null value shows no badge. */
  detected_language?: string | null;
  /** Tags associated with this recording */
  tags?: Array<{ id: number; name: string; color?: string | null }>;
  /** Custom display names for this recording's diarized speaker labels, e.g.
   *  `[Speaker 1]` → "Sarah". Applied at display/export time; the stored
   *  transcript keeps its `[Speaker N]` markers. Empty when none are set. */
  speaker_names?: SpeakerName[];
};

/** A custom display name for one diarized speaker label within a recording.
 *  `speaker_label` is the 1-based index from a `[Speaker N]` marker. */
export type SpeakerName = { speaker_label: number; name: string };

/** One structured, typed entity extracted from a recording's transcript.
 *  `kind` is `person` | `org` | `topic` | `term` (an unknown kind from the model
 *  is stored as `topic`); `value` is the surface text. */
export type Entity = { kind: string; value: string };

/** One task / action item extracted from a recording's transcript by the
 *  task-extraction step. `id` is the table row id (needed to toggle `done`);
 *  `text` is the action; `due_hint` is the model's free-text deadline phrase
 *  ("by Friday") stored verbatim (never a parsed date), null when none; `done`
 *  is the one mutable, user-owned field. Task counterpart of {@link Entity}, plus
 *  the mutable `done` + free-text `due_hint`. */
export type Task = {
  id: number;
  text: string;
  due_hint?: string | null;
  done: boolean;
};

/** A whole-meeting digest: one LLM synthesis across all of a meeting's tracks
 *  (mic + system together), distinct from a single track's `summary`. Keyed by
 *  `meeting_id`; `digest_model` is the LLM that produced it (null when unknown). */
export type MeetingDigest = {
  meeting_id: string;
  digest: string;
  digest_model?: string | null;
};

/** How a recording started via the `record_start` command decides to stop: "hold" = on the
 *  explicit stop signal (Stop click / hotkey release), "oneshot" = by itself
 *  on silence (or the max-duration ceiling), `duration:N` = after exactly N
 *  seconds. See services/recordStopMode.ts for the UI-level mapping. */
export type RecordMode = "hold" | "oneshot" | `duration:${number}`;

/** One user-defined global shortcut in `config.hotkeys` — the extra keybinds
 *  beyond the three built-ins (record / meeting / in-place). Each fires app-wide
 *  via the tray's global-shortcut registration. `id` is stable (generated once);
 *  `combo` is a tauri-plugin-global-shortcut string (e.g. "Ctrl+Alt+E"); `mode`
 *  is hold (push-to-talk) vs. toggle; `action` is which capture it triggers. */
export type HotkeyBinding = {
  id: string;
  label: string;
  enabled: boolean;
  combo: string;
  mode: "hold" | "toggle";
  action: "record" | "in_place" | "meeting";
  /** The Playbook recipe this keybind's recordings run, by {@link PlaybookRecipe.id}.
   *  Empty = the global `default` recipe (today's normal-recording pipeline), so
   *  existing bindings are unchanged; a non-empty id points the daemon at that
   *  recipe instead. Supersedes the legacy `pipeline` flags below. */
  recipe_id: string;
  /** Per-keybind transcription (Whisper/STT) model override. Empty = the globally
   *  configured model; a non-empty value transcribes this keybind's recordings
   *  with that model (a model-file path for the local backend, a model id for
   *  cloud backends). */
  whisper_model: string;
  /** Per-keybind capture-source override. `null`/absent = follow the global
   *  `[recording].source`; otherwise this keybind's recordings capture the
   *  microphone or system audio regardless of the global setting. Ignored for
   *  `meeting` bindings (a meeting always records both tracks). */
  source?: "microphone" | "system_audio" | null;
  /** In-place-dictation options (only used when action === "in_place"):
   *  fast type-only vs. run the pipeline first, and how to insert the text. */
  in_place: { full_pipeline: boolean; type_mode: "type" | "paste" | "off" };
};

/** What a {@link PlaybookEntry} does (mirrors the Rust `PlaybookKind`):
 *  `transform` rewrites the running transcript text via an LLM; `filler_removal`
 *  does the same deterministically (no AI, reads `[filler]`); `enrichment` writes
 *  a named field (see `target`); `hook` fires a command/webhook. */
export type PlaybookKind = "transform" | "filler_removal" | "enrichment" | "hook";

/** The LLM half of a Playbook entry (used for transform/enrichment). The API key
 *  is resolved from the matching provider section at run time, never stored here. */
export type PlaybookLlm = {
  /** Provider id; empty = inherit the default Post-Processing connection. */
  provider: string;
  model: string;
  prompt: string;
  api_url: string;
  /** Per-entry API key (masked to the WebView like other key fields). Empty
   *  when the entry inherits the default connection. */
  api_key: string;
  timeout_secs: number;
};

/** The hook half of a Playbook entry (used for `kind: "hook"`). */
export type PlaybookHook = {
  command: string;
  webhook_url: string;
  timeout_secs: number;
  /** Trigger: when non-empty, only run if the transcript contains this substring
   *  (the Playbook-native keyword rule). Empty/absent = always run. */
  keyword?: string;
  case_sensitive?: boolean;
  /** When true, a failure of this hook fails the whole recording (default false:
   *  surfaced but non-fatal). */
  required?: boolean;
};

/** One reusable "move" in the Playbook (`config.playbook`) — a curated or custom
 *  LLM/hook step. `kind` selects which sub-object applies; `target` (enrichment
 *  only) is `title` | `summary` | `tags` | `custom:<key>`. `builtin` entries are
 *  seeded by Phoneme (editable; resettable to their seed). */
export type PlaybookEntry = {
  id: string;
  name: string;
  description: string;
  builtin: boolean;
  kind: PlaybookKind;
  llm: PlaybookLlm;
  target: string;
  hook: PlaybookHook;
};

/** What a {@link PlaybookRecipe} runs over: a single recording (the default) or a
 *  whole meeting's merged transcript (a "meeting template"). Mirrors the Rust
 *  `RecipeScope`. */
export type RecipeScope = "recording" | "meeting";

/** A named, ordered chain of {@link PlaybookEntry} ids (`config.recipes`) — what
 *  the default recording pipeline and Custom Hotkeys run. `scope` distinguishes a
 *  per-recording recipe from a meeting template (one pass over the merged
 *  transcript); omitted/`"recording"` for every existing recipe. */
export type PlaybookRecipe = {
  id: string;
  name: string;
  description: string;
  builtin: boolean;
  /** Defaults to `"recording"` when absent (the Rust serde default). */
  scope?: RecipeScope;
  steps: string[];
};

/** Server-side query filter for `listRecordings` (wire shape — snake_case
 *  fields, applied in SQL before pagination). The UI builds it from the
 *  richer `UiFilter` via `state/filter.ts` `toWireFilter`. */
export type ListFilter = {
  limit?: number | null;
  /** Rows to skip before returning results (pagination; pairs with `limit`). */
  offset?: number | null;
  since?: string | null;
  until?: string | null;
  status?: string | null;
  search?: string | null;
  tag_id?: number | null;
  /** `true` (default) = newest first; `false` = oldest first. */
  sort_desc?: boolean | null;
  /** Server-side recording-type filter: only single voice notes, or only
   *  meeting tracks. Applied in SQL before pagination, so pages stay full.
   *  Omit for all kinds (the UI's "favorite" choice maps to `favorite`). */
  kind?: "single" | "meeting" | null;
  /** Server-side favorites flag: `true` = starred only, `false` = unstarred
   *  only, omit = no filter. Applied in SQL before pagination. */
  favorite?: boolean | null;
  /** Server-side pinned flag: `true` = pinned only, `false` = unpinned only,
   *  omit/null = no filter. Applied in SQL before pagination. Independent of the
   *  pinned-first sort the daemon always applies. */
  pinned?: boolean | null;
  /** Server-side in-place-dictation flag: `true` = only recordings captured via
   *  in-place dictation, omit = no filter. Applied in SQL before pagination. */
  in_place?: boolean | null;
  /** Server-side tag-presence filter: `true` = only recordings with at least one
   *  tag, `false` = only recordings with no tags, omit/null = no filter. Applied
   *  in SQL before pagination. Independent of `tag_id` (which scopes to a single
   *  tag). Powers the sidebar's "Untagged" / "Tagged" rows. */
  tagged?: boolean | null;
  /** Server-side entity facet filter: keep only recordings that mention this exact
   *  entity `value` (the cross-recording browse-by-entity surface, the entity
   *  counterpart of `tag_id`). Applied in SQL before pagination via a subquery
   *  over the `entities` child table. */
  entity_value?: string | null;
  /** The entity facet filter's `kind` (`person`/`org`/`topic`/`term`), pairing
   *  with `entity_value` so the same surface text under two kinds is distinct.
   *  Ignored unless `entity_value` is set; omit to match the value across kinds. */
  entity_kind?: string | null;
  /** Server-side task-presence filter (the sidebar's Tasks section): `"has_open"`
   *  = only recordings with at least one not-done task, `"has_tasks"` = any
   *  extracted task, omit/null = no filter. Applied in SQL before pagination via a
   *  subquery over the `tasks` child table. An unrecognized value is ignored. */
  task_state?: string | null;
  /** Server-side low-confidence filter: when set, only recordings whose stored
   *  `mean_confidence` is non-null AND strictly below this value. Applied in SQL
   *  before pagination. The value is the configured
   *  `[whisper].low_confidence_threshold`; the UI sets it (via the sidebar
   *  "Low confidence" row) rather than letting the user pick the number. A null
   *  aggregate never matches, so older rows / cloud transcripts are excluded. */
  low_confidence_below?: number | null;
};

/**
 * Fetches a list of recordings matching the given filter.
 * The results are paginated or limited by the backend (default limit 50).
 */
export async function listRecordings(filter: ListFilter = {}): Promise<Recording[]> {
  return await tauriInvoke<Recording[]>("list_recordings", { filter });
}

/** Fetch one recording by id (rejects if it doesn't exist). The standard
 *  "re-fetch on event" call: most `*_updated` daemon events carry only an id
 *  and expect listeners to reload the row through here. */
export async function getRecording(id: string): Promise<Recording> {
  return await tauriInvoke<Recording>("get_recording", { id });
}

/** One persisted AI-activity session (a completed cleanup/summary LLM run), as
 *  stored in the durable log that survives app restarts. `stage` is the
 *  PipelineStage wire value (snake_case) so `stageLabel()` renders it. */
export type AiActivityEntry = {
  id: number;
  recording_id: string;
  stage: string;
  prompt: string;
  response: string;
  created_at: string;
};

/** Recent persisted AI-activity sessions, newest first. With `recordingId` set,
 *  only that recording's sessions; otherwise the whole library's recent activity.
 *  Powers the 🧠 popout's history so it isn't empty after an app restart. */
export async function listAiActivity(
  recordingId?: string,
  limit = 200,
): Promise<AiActivityEntry[]> {
  return await tauriInvoke<AiActivityEntry[]>("list_ai_activity", {
    recordingId: recordingId ?? null,
    limit,
  });
}

/** One machine transcript segment with its audio-relative timing.
 *  `start_ms`/`end_ms` are offsets into the track's audio file; `speaker` is
 *  the label exactly as it appears in the transcript's `[Speaker …]` marker
 *  ("1", "0", "A" — providers differ; numeric ones map onto `speaker_names`),
 *  or null for undiarized segments. Machine truth: user edits to the live
 *  transcript never rewrite these. */
export type TranscriptSegment = {
  start_ms: number;
  end_ms: number;
  text: string;
  speaker?: string | null;
};

/** A recording's machine transcript segments in timeline order. An empty list
 *  is a normal state (older recordings predate segment capture; some providers
 *  return no timing data), so callers should fall back to the plain transcript
 *  rather than treating it as an error. */
export async function getSegments(
  id: string,
  variant?: "raw" | "cleaned",
): Promise<TranscriptSegment[]> {
  return await tauriInvoke<TranscriptSegment[]>("get_segments", { id, variant });
}

/** One auto-chapter: a time range over a recording's transcript plus a short
 *  title (and an optional one-line summary), derived by the LLM from the
 *  segment timing. `start_ms`/`end_ms` are offsets into the track's audio; the
 *  daemon anchors each `start_ms` to a real segment start, so a chapter row
 *  lines up with the audio. */
export type Chapter = {
  start_ms: number;
  end_ms: number;
  title: string;
  summary?: string | null;
};

/** A recording's auto-chapters in chronological order. An empty list is a
 *  normal state (no timing to chapter, or the auto-chapter step never ran), so
 *  the view shows a generate affordance rather than treating it as an error. */
export async function getChapters(id: string): Promise<Chapter[]> {
  return await tauriInvoke<Chapter[]>("get_chapters", { id });
}

/** One recording's structured entities (person / org / topic / term), kind- then
 *  value-sorted. The detail-pane entity chips use this instead of pulling the
 *  whole {@link getRecording} row just for its `entities`. An empty list is a
 *  normal state (no entities extracted yet). */
export async function getEntities(id: string): Promise<Entity[]> {
  return await tauriInvoke<Entity[]>("get_entities", { id });
}

/** One machine transcript word with its audio-relative timing.
 *  `idx` is the 0-based timeline order across the whole recording;
 *  `start_ms`/`end_ms` are offsets into the track's audio file; `speaker`
 *  mirrors the owning segment's `[Speaker …]` label (or null when undiarized);
 *  `confidence` is 0..1 when the provider reports it, else null (whisper-family
 *  cloud, native whisper, and older recordings give none). Machine truth: user
 *  edits to the live transcript never rewrite these. */
export type TranscriptWord = {
  idx: number;
  start_ms: number;
  end_ms: number;
  text: string;
  speaker?: string | null;
  confidence?: number | null;
  /** Whether this token starts a new written word (whisper's leading-space
   *  marker). When false — punctuation, a clitic ("'s"), or a subword piece —
   *  the Synced view joins it to the previous word WITHOUT a space, so
   *  "over"+"ste"+"pped" renders "overstepped" and "weapon"+"?" renders
   *  "weapon?". Absent on older words → treated as true (space-separated). */
  leading_space?: boolean;
};

/** A recording's machine transcript words in timeline order. Like segments, an
 *  empty list is a normal state — older recordings predate word capture, and
 *  several providers return no per-word timing — so the synced-transcript view
 *  treats it as "no word timings" rather than an error. */
export async function getWords(
  id: string,
  variant?: "raw" | "cleaned",
): Promise<TranscriptWord[]> {
  return await tauriInvoke<TranscriptWord[]>("get_words", { id, variant });
}

/** One step's transcript in the compounding chain (PB-COMPOUND): `idx` 0 = the
 *  raw ASR, then each Transform step's output (the last being the live
 *  transcript). Powers the Compare-versions step chain + revert. */
export type TranscriptVersion = {
  idx: number;
  step_id?: string | null;
  label?: string | null;
  model?: string | null;
  text: string;
};

/** A recording's compounding chain in step order. Empty for a plain transcribe
 *  (no Transform step ran) — callers treat that as "no chain", not an error. */
export async function listTranscriptVersions(id: string): Promise<TranscriptVersion[]> {
  return await tauriInvoke<TranscriptVersion[]>("list_transcript_versions", { id });
}

/** Revert the live transcript to a recorded version (by step `idx`). Routes
 *  through the same path as a manual edit (re-flows timing + re-embeds); the
 *  daemon emits `transcript-updated`. */
export async function revertToVersion(id: string, idx: number): Promise<void> {
  await tauriInvoke("revert_to_version", { id, idx });
}

/** One semantic-search hit: the recording plus its similarity score
 *  (cosine-derived, 0..1, higher = more relevant). */
export interface SemanticSearchResult {
  recording: Recording;
  score: number;
}

/** Meaning-based search: embed `query` and rank recordings by vector
 *  similarity, best first. Needs the semantic index (the daemon embeds
 *  transcripts as they complete); rejects when the embedding model is
 *  unavailable. The header's ✨ toggle routes searches here instead of the
 *  FTS path inside `listRecordings`. */
export async function semanticSearch(query: string, limit: number = 20): Promise<SemanticSearchResult[]> {
  return await tauriInvoke<SemanticSearchResult[]>("semantic_search", { query, limit });
}

/** "More like this": recordings semantically similar to a stored one, scored
 *  from its already-stored vectors (no fresh query embedding — works even when
 *  the embedding model isn't loaded). Same result shape as `semanticSearch`;
 *  the source recording (and the other track of its own meeting) is never
 *  included. Rejects with a clear "isn't indexed yet" message when the
 *  recording has no embeddings. */
export async function moreLikeThis(id: string, limit: number = 20): Promise<SemanticSearchResult[]> {
  return await tauriInvoke<SemanticSearchResult[]>("more_like_this", { id, limit });
}

/** Clear all embeddings and re-embed the whole library with the current model.
 *  Run after changing the embedding model. Returns at once; the daemon
 *  re-embeds in the background. */
export async function reembedAll(): Promise<void> {
  await tauriInvoke("reembed_all");
}

/**
 * Ask-my-archive (local RAG): answer `query` from the user's own transcripts,
 * grounded with citations. The daemon ACKs at once and streams the answer over
 * `ask_activity` daemon events tagged with `requestId` (sources first, then
 * `delta` chunks, then a terminal `done`). The caller mints `requestId`
 * (`crypto.randomUUID()`) BEFORE subscribing so it can filter the shared event
 * stream with no race. `topK` caps the grounding chunks (clamped server-side);
 * `filter` scopes the answer to a Library subset (same shape as
 * `semanticSearch`), or omit for the whole library. The promise resolves on the
 * ACK — the answer arrives on the event stream, not here.
 */
export async function ask(
  requestId: string,
  query: string,
  topK = 8,
  filter: ListFilter | null = null,
): Promise<void> {
  await tauriInvoke("ask", { requestId, query, topK, filter });
}

/**
 * Fetch all recordings belonging to a single meeting session (the two tracks
 * sharing a `meeting_id`), ordered by track then start time.
 */
export async function listSession(meetingId: string): Promise<Recording[]> {
  return await tauriInvoke<Recording[]>("list_meeting", { meetingId });
}

/**
 * Fetch a meeting's whole-meeting digest (the LLM synthesis across all of its
 * tracks), or `null` when none has been generated yet. The merged meeting view
 * fetches this alongside {@link listSession}.
 */
export async function getMeetingDigest(meetingId: string): Promise<MeetingDigest | null> {
  return await tauriInvoke<MeetingDigest | null>("get_meeting_digest", { meetingId });
}

/**
 * Generate (or regenerate) a meeting's whole-meeting digest on demand — one LLM
 * synthesis across every track (mic + system together), distinct from a single
 * track's {@link rerunSummary}. Reuses the configured summary provider; `model`
 * overrides the summary model for this run only (never persisted). `recipeId`, when
 * set, runs a specific meeting template (a `scope: "meeting"` recipe) for this run
 * only instead of the configured one. The digest arrives via the
 * `MeetingDigestUpdated` daemon event — re-fetch when it fires.
 */
export async function rerunMeetingDigest(
  meetingId: string,
  model: string | null = null,
  recipeId: string | null = null,
  provider: string | null = null,
  apiUrl: string | null = null,
  apiKey: string | null = null,
): Promise<void> {
  await tauriInvoke("rerun_meeting_digest", { meetingId, model, recipeId, provider, apiUrl, apiKey });
}

/**
 * Deletes a recording by ID. If keepAudio is true, the catalog entry is removed
 * but the raw `.wav` file is preserved on disk.
 */
export async function deleteRecording(id: string, keepAudio = false): Promise<void> {
  await tauriInvoke("delete_recording", { id, keepAudio });
}

/**
 * Delete a whole meeting session — every track sharing `meetingId` — in one
 * call. Each track emits its own `recording_deleted` event so views drop them.
 * If `keepAudio` is false the tracks' `.wav` files are also removed.
 */
export async function deleteSession(meetingId: string, keepAudio = false): Promise<void> {
  await tauriInvoke("delete_session", { meetingId, keepAudio });
}

/** Set (or clear, with `null`) the display name of a meeting session. Shown
 *  on the list's group header; the tracks themselves are untouched. */
export async function updateMeetingName(meetingId: string, name: string | null): Promise<void> {
  await tauriInvoke("update_meeting_name", { meetingId, name });
}

/**
 * One-time whole-pipeline overrides for a Re-run → "All". Keys are snake_case
 * to match the daemon's `RerunAllOverrides` (Tauri only camelCases the top-level
 * command args, not nested object keys). The API key is intentionally absent —
 * cleanup/summary reuse the configured key. When present, cleanup + auto-summary
 * are forced on for this one run.
 */
export type RerunAllOverrides = {
  cleanup_provider?: string | null;
  cleanup_model?: string | null;
  cleanup_prompt?: string | null;
  cleanup_api_url?: string | null;
  summary_model?: string | null;
  summary_prompt?: string | null;
  /** Auto-title model for this run only. When set, the title step runs with an
   *  LLM using this model (enabled for the run even if globally off). */
  title_model?: string | null;
};

/**
 * Re-run the whole pipeline on a recording's stored audio. Each `null` means
 * "use the configured default": `model` overrides the transcription model for
 * this run, `runHooks`/`postProcess` force the hook / cleanup steps on or off,
 * and `allOverrides` (Re-run → "All") additionally overrides the cleanup +
 * summary settings one-time. Returns as soon as the job is queued — progress
 * arrives as the normal pipeline events.
 */
export async function retranscribeRecording(
  id: string,
  model: string | null = null,
  runHooks: boolean | null = null,
  postProcess: boolean | null = null,
  allOverrides: RerunAllOverrides | null = null,
  recipeId: string | null = null,
): Promise<void> {
  await tauriInvoke("retranscribe_recording", { id, model, runHooks, postProcess, allOverrides, recipeId });
}

/**
 * Import an existing audio file (wav/mp3/m4a/flac). The daemon decodes it to a
 * canonical WAV and transcribes it like a normal recording. Returns the new id.
 */
export async function importRecording(path: string): Promise<{ id: string }> {
  return await tauriInvoke<{ id: string }>("import_recording", { path });
}

/** Safe re-import: re-link audio files on disk that have no catalog row. With
 *  `dryRun`, only counts (`{ count, paths }`); otherwise inserts + enqueues them
 *  (`{ count }`). Non-destructive — never deletes or touches existing rows. */
export async function reimportFromDisk(
  dryRun: boolean,
): Promise<{ count: number; paths?: string[] }> {
  return await tauriInvoke<{ count: number; paths?: string[] }>("reimport_from_disk", { dryRun });
}

/**
 * Destructive catalog rebuild from disk: clears every recording (losing
 * transcripts, edits, tags) and re-imports each WAV as a fresh `Queued`
 * recording. The daemon does it in-process and refuses while a recording is
 * active. Returns the number of recordings re-imported.
 */
export async function rebuildCatalog(): Promise<{ count: number }> {
  return await tauriInvoke<{ count: number }>("rebuild_catalog", {});
}

/** File extensions accepted by the import flow (no leading dot). */
export const IMPORT_AUDIO_EXTENSIONS = ["wav", "mp3", "m4a", "flac"] as const;

/** Re-run the post-transcription hook(s) for a recording without touching the
 *  transcript. `command` overrides the configured hook command for this run
 *  only; `null` re-fires the configured ones. `hook_started` / `hook_done` /
 *  `hook_failed` events report the outcome. */
export async function refireHook(id: string, command: string | null = null): Promise<void> {
  await tauriInvoke("refire_hook", { id, command });
}

/** Result of running a hook command against a representative sample payload.
 *  `stderr_tail` is secret-redacted by the daemon before it crosses the pipe. */
export type HookTestResult = {
  exit_code: number;
  duration_ms: number;
  stderr_tail: string;
};

/** Run one hook command against a sample payload — the Hook Manager's "Test"
 *  affordance for a command the user is still editing. `command` is the command
 *  to test; `null` tests the first configured hook. A non-zero `exit_code`
 *  resolves normally (the run completed but the command failed); only a launch
 *  failure rejects. */
export async function hookTest(command: string | null = null): Promise<HookTestResult> {
  return await tauriInvoke<HookTestResult>("hook_test", { command });
}

/** Literal find-and-replace across one recording's live transcript. Re-flows
 *  word/segment timing and re-embeds the result; the original/clean baselines
 *  are preserved so the edit stays revertible. Returns the number of
 *  occurrences replaced (`0` = no match, a true no-op). A change arrives via the
 *  `TranscriptUpdated` daemon event. */
export async function findReplace(
  id: string,
  find: string,
  replace: string,
  caseSensitive = false,
): Promise<{ replaced: number }> {
  return await tauriInvoke<{ replaced: number }>("find_replace", {
    id,
    find,
    replace,
    caseSensitive,
  });
}

/** Library-wide literal find-and-replace — the same substring replacement run
 *  over every recording's live transcript. Each changed recording is re-flowed
 *  and re-embedded (heavy: the daemon ACKs with the counts immediately and runs
 *  the re-embed pass in the background, emitting one `TranscriptUpdated` per
 *  changed recording). Recordings with no match are skipped. Returns the count
 *  of recordings changed, total occurrences replaced, and the number of
 *  recordings whose rewrite errored. */
export async function findReplaceLibrary(
  find: string,
  replace: string,
  caseSensitive = false,
): Promise<{ recordings_changed: number; total_replacements: number; failed: number }> {
  return await tauriInvoke<{
    recordings_changed: number;
    total_replacements: number;
    failed: number;
  }>("find_replace_library", { find, replace, caseSensitive });
}

/**
 * Re-run only the LLM post-processing ("cleanup") step on a recording's stored
 * transcript, without re-transcribing the audio. The preserved original
 * (machine) transcript is used as the input, so the original is never lost.
 * Each override applies to this run only and is never written back to config;
 * `null` falls back to the configured `[llm_post_process]` value. Supplying a
 * `provider` also forces cleanup on for this run.
 */
export async function rerunCleanup(
  id: string,
  model: string | null = null,
  provider: string | null = null,
  prompt: string | null = null,
  apiUrl: string | null = null,
  apiKey: string | null = null,
): Promise<void> {
  await tauriInvoke("rerun_cleanup", { id, model, provider, prompt, apiUrl, apiKey });
}

/**
 * Generate (or regenerate) an LLM summary of a recording's current transcript
 * on demand, and store it. `model`/`prompt` override the configured summary
 * model/prompt for this run only; `provider`/`apiUrl`/`apiKey` override the
 * connection for this run only (mirroring {@link rerunCleanup}). `null` falls
 * back to the configured summary / `[llm_post_process]` connection; a masked
 * `apiKey` ({@link MASKED_SECRET}) means "use the configured key" (resolved in
 * Rust — the real secret never round-trips through the WebView). None of it is
 * persisted. The summary arrives via the `SummaryUpdated` daemon event.
 */
export async function rerunSummary(
  id: string,
  model: string | null = null,
  prompt: string | null = null,
  provider: string | null = null,
  apiUrl: string | null = null,
  apiKey: string | null = null,
): Promise<void> {
  await tauriInvoke("rerun_summary", { id, model, prompt, provider, apiUrl, apiKey });
}

/** One entry in the transcription pipeline queue. */
export type QueueEntry = {
  id: string;
  timestamp: string;
  audio_path: string;
  duration_ms: number;
  model: string;
  /** "processing" = actively transcribing; "pending" = waiting in line. */
  state: "pending" | "processing";
};

/** List the transcription pipeline queue (processing item(s) first, then pending). */
export async function listQueue(): Promise<QueueEntry[]> {
  return await tauriInvoke<QueueEntry[]>("list_queue");
}

/** Remove a still-pending recording from the queue. */
export async function cancelQueued(id: string): Promise<void> {
  await tauriInvoke("cancel_queued", { id });
}

/** Set the pending queue's claim order (full ordered list of recording ids). */
export async function reorderQueue(ids: string[]): Promise<void> {
  await tauriInvoke("reorder_queue", { ids });
}

/** Pause or resume the transcription queue. Returns the new paused state. */
export async function setQueuePaused(paused: boolean): Promise<boolean> {
  const r = await tauriInvoke<{ paused: boolean }>("set_queue_paused", { paused });
  return r.paused;
}

/** Whether the transcription queue is currently paused. */
export async function queuePaused(): Promise<boolean> {
  const r = await tauriInvoke<{ paused: boolean }>("queue_paused");
  return r.paused;
}

/** Inbox depth counts. `failed` = items quarantined in the inbox `failed/`
 *  folder (permanent transcription/hook errors, corrupt payloads, cancels). */
export type QueueCounts = { pending: number; processing: number; done: number; failed: number };

/** Fetch the current inbox depth counts on demand (accurate on a fresh load,
 *  unlike the event-only path which a webview reload would miss). */
export async function getQueueCounts(): Promise<QueueCounts> {
  return await tauriInvoke<QueueCounts>("queue_counts");
}

/** Clear the inbox `failed/` quarantine ("dismiss failed"). Returns the count
 *  removed. Catalog rows keep their failed status — only the inbox is emptied. */
export async function clearFailed(): Promise<number> {
  const r = await tauriInvoke<{ removed: number }>("clear_failed");
  return r.removed;
}

/** Dismiss a single item from the inbox `failed/` quarantine by id (the per-item
 *  counterpart to {@link clearFailed}). Returns whether a file was removed.
 *  The catalog row keeps its failed status; only the inbox file is removed. */
export async function dismissFailed(id: string): Promise<boolean> {
  const r = await tauriInvoke<{ removed: boolean }>("dismiss_failed", { id });
  return r.removed;
}

/** Wire shape of a saved search as the catalog stores it — `filter_json` is the
 *  opaque serialized `UiFilter` (the state module parses/serializes it). */
export type SavedSearchRow = { id: string; name: string; filter_json: string };

/** All saved searches (catalog-backed), most-recently-updated first. */
export async function listSavedSearches(): Promise<SavedSearchRow[]> {
  return await tauriInvoke<SavedSearchRow[]>("list_saved_searches");
}

/** Insert or update a saved search by id; `filterJson` is a serialized `UiFilter`. */
export async function upsertSavedSearch(
  id: string,
  name: string,
  filterJson: string,
): Promise<void> {
  await tauriInvoke("upsert_saved_search", { id, name, filterJson });
}

/** Delete a saved search by id; resolves to whether a row was removed. */
export async function deleteSavedSearch(id: string): Promise<boolean> {
  const r = await tauriInvoke<{ removed: boolean }>("delete_saved_search", { id });
  return r.removed;
}

/** Wire shape of a stored in-place dictation (the opt-in re-grab history). `text`
 *  is the text *as typed* at the cursor (not the raw transcript); `app` is the
 *  focused app's exe stem at type time when known. */
export type DictationHistoryRow = {
  id: number;
  text: string;
  char_count: number;
  app: string | null;
  created_at: string;
};

/** Recent in-place dictations (the typed text), newest first. Empty unless
 *  `[in_place].keep_history` is on. */
export async function listDictationHistory(limit = 50): Promise<DictationHistoryRow[]> {
  return await tauriInvoke<DictationHistoryRow[]>("list_dictation_history", { limit });
}

/** Re-insert a past dictation's text at the CURRENT cursor (it lands wherever the
 *  caret is now). `mode` is `"type"`/`"paste"`, or omit for the configured
 *  `type_mode`. Injects real keystrokes/paste — verify in the native window. */
export async function regrabDictation(id: number, mode?: "type" | "paste"): Promise<void> {
  await tauriInvoke("regrab_dictation", { id, mode: mode ?? null });
}

/** Forget one dictation from the history by id; resolves to whether a row was
 *  removed. */
export async function deleteDictationHistory(id: number): Promise<boolean> {
  const r = await tauriInvoke<{ removed: boolean }>("delete_dictation_history", { id });
  return r.removed;
}

/** Clear the whole dictation history; resolves to how many rows were removed. */
export async function clearDictationHistory(): Promise<number> {
  const r = await tauriInvoke<{ removed: number }>("clear_dictation_history");
  return r.removed;
}

/** Tail the last `maxLines` of a daemon log (`hook.log` / `daemon.log` /
 *  `ollama.log`) for the in-app log viewer. Returns "" when the log doesn't
 *  exist yet. The basename is allowlisted on the backend. */
export async function tailLog(name: string, maxLines = 200): Promise<string> {
  return await tauriInvoke<string>("tail_log", { name, maxLines });
}

/** Remove every still-pending item from the queue. Returns how many were removed. */
export async function cancelAllQueued(): Promise<number> {
  const r = await tauriInvoke<{ removed: number }>("cancel_all_queued");
  return r.removed;
}

/** Cancel the item currently being processed (aborts the in-flight work). */
export async function cancelProcessing(id: string): Promise<void> {
  await tauriInvoke("cancel_processing", { id });
}

/** One Doctor health-check result. */
export type DoctorCheck = {
  name: string;
  ok: boolean;
  detail: string;
  /** Opaque token the GUI maps to a "Fix" action (e.g. open_config). */
  fix_action?: string | null;
};

/** Run all health checks (local + backend reachability) for the Doctor view. */
export async function runDoctor(): Promise<DoctorCheck[]> {
  return await tauriInvoke<DoctorCheck[]>("run_doctor");
}

/**
 * Manually update the text transcript of a specific recording.
 */
export async function updateTranscript(id: string, text: string): Promise<void> {
  await tauriInvoke("update_transcript", { id, text });
}

/** The preserved original (machine) transcript, or null if none was saved. */
export async function getOriginalTranscript(id: string): Promise<string | null> {
  return await tauriInvoke<string | null>("get_original_transcript", { id });
}

/**
 * The preserved "unedited" transcript — the pipeline output (transcribed +
 * cleaned) before the user made any hand edits. `null` if none was saved (e.g.
 * recordings transcribed before this was tracked).
 */
export async function getCleanTranscript(id: string): Promise<string | null> {
  return await tauriInvoke<string | null>("get_clean_transcript", { id });
}

/**
 * Update the free-form user notes for a recording. Notes are stored separately
 * from the transcript and are never affected by (re-)transcription.
 */
export async function updateNotes(id: string, notes: string): Promise<void> {
  await tauriInvoke("update_notes", { id, notes });
}

/** Star or unstar a recording (the Favorites view). Cosmetic organisation only. */
export async function setFavorite(id: string, favorite: boolean): Promise<void> {
  await tauriInvoke("set_favorite", { id, favorite });
}

/** Pin or unpin a recording (the Pinned view). Pinned recordings sort to the top
 *  of the library, independent of the favorite flag. Cosmetic organisation only. */
export async function setPinned(id: string, pinned: boolean): Promise<void> {
  await tauriInvoke("set_pinned", { id, pinned });
}

/**
 * Set or clear a recording's display title. A non-empty string marks the title
 * user-owned — auto generation never overwrites it again. `null` (or empty)
 * clears it back to auto: the title empties now and is regenerated on the next
 * pipeline run (e.g. a retranscribe). A `transcript_updated` event fires so
 * open views refresh.
 */
export async function setRecordingTitle(id: string, title: string | null): Promise<void> {
  await tauriInvoke("set_recording_title", { id, title });
}

/** Caption export formats `exportCaptions` understands (no leading dot). */
export type CaptionFormat = "srt" | "vtt";

/**
 * Render a recording's machine segments as caption text in the chosen format
 * ("srt" or "vtt"), returning the body for the caller to drop into a save
 * dialog (the command writes no file — the dialog owns the destination). The
 * format→content mapping lives in `phoneme_core::export`, so the GUI captions
 * match `phoneme export --captions` byte for byte. Rejects with a `not_found`
 * error carrying "no segments stored — retranscribe…" when the recording has
 * no segments, so callers surface the same hint the CLI gives instead of
 * saving an empty file.
 */
export async function exportCaptions(id: string, format: CaptionFormat): Promise<string> {
  return await tauriInvoke<string>("export_captions", { id, format });
}

/**
 * Bundle one recording's full data — its catalog row plus machine segments —
 * into a pretty-printed JSON string for "Export → All data". Returns the body
 * for {@link saveTextExport}; segments are best-effort (empty for recordings
 * transcribed before segment capture existed).
 */
export async function exportRecordingJson(id: string): Promise<string> {
  return await tauriInvoke<string>("export_recording_json", { id });
}

/**
 * Write `contents` to `dest` (a save-dialog path) on the daemon-side bridge
 * process — the single write path for every per-recording export. The WebView
 * never needs the `fs` plugin's write permission for an arbitrary path (which
 * `fs:default` denies). Produce the text (transcript / {@link exportCaptions} /
 * {@link exportRecordingJson}), then hand it here with the chosen destination.
 */
export async function saveTextExport(dest: string, contents: string): Promise<void> {
  await tauriInvoke("save_text_export", { dest, contents });
}

/**
 * Write a portable backup of the whole library to `dest` (a `.zip` path picked
 * via the save dialog). Mirrors `phoneme export <FILE>`: a `catalog.json`
 * envelope (recordings + tags) plus every `.wav` under the audio dir packed
 * into `audio/`. Distinct from Settings → Storage's plain JSON/CSV/TXT
 * "Export All", which carries no audio. Returns the number of audio files
 * packed.
 */
export async function exportLibraryZip(dest: string): Promise<number> {
  return await tauriInvoke<number>("export_library_zip", { dest });
}

/**
 * Export a time range of a recording's audio to a new WAV (the GUI counterpart
 * of `phoneme clip <ID> <START> <END> [OUT]`). `startMs`/`endMs` are
 * milliseconds from the recording's start; the daemon slices `[start, end)` on
 * sample-frame boundaries and clamps `end` to the recording's duration. Pass
 * `outPath: null` (the default) to let the daemon pick the sibling
 * `_clip_<start>-<end>.wav` path next to the source, matching the CLI. Returns
 * the path of the WAV that was written. Rejects with the structured command
 * error on failure (e.g. the range collapsed to nothing, or the source audio is
 * gone) — the caller toasts it.
 */
export async function exportClip(
  id: string,
  startMs: number,
  endMs: number,
  outPath: string | null = null,
): Promise<{ path: string }> {
  return await tauriInvoke<{ path: string }>("export_clip", { id, startMs, endMs, outPath });
}

/** Edit a recording's audio (#262): keep only `keepRanges` (`[startMs, endMs)`
 *  pairs, ascending + non-overlapping) and concatenate them — a trim is one
 *  range, deleting an inner section is the gap between two. `newRecording=true`
 *  saves the result as a NEW recording (original untouched, returns its id);
 *  `false` replaces this recording's audio in place (original backed up) and
 *  re-transcribes (returns the same id + the backup path). Rejects with the
 *  structured command error on a bad range / IO failure — the caller toasts it. */
export async function editRecording(
  id: string,
  keepRanges: [number, number][],
  newRecording: boolean,
): Promise<{ id: string; backup?: string }> {
  return await tauriInvoke<{ id: string; backup?: string }>("edit_recording", {
    id,
    keepRanges,
    newRecording,
  });
}

/** Skip the LLM step (cleanup / summary / tagging) currently running for the
 *  active queue item; the pipeline continues with the next step. */
export async function skipCurrentStage(): Promise<void> {
  await tauriInvoke("skip_current_stage");
}

/** Ask the LLM to suggest tags for a recording now (on demand). Suggestions
 *  land on the recording; a `tag_suggestions_updated` event fires when ready. */
export async function suggestTags(id: string): Promise<void> {
  await tauriInvoke("suggest_tags", { id });
}

/** Ask the LLM to extract structured entities for a recording now (on demand).
 *  The typed entities land on the recording; an `entities_updated` event fires
 *  when ready. Entity counterpart of {@link suggestTags}. */
export async function suggestEntities(id: string): Promise<void> {
  await tauriInvoke("suggest_entities", { id });
}

/** Ask the LLM to generate topic chapters for a recording now (on demand). The
 *  time-ranged chapters land on the recording; a `chapters_updated` event fires
 *  when ready. Chapter counterpart of {@link suggestEntities}. */
export async function suggestChapters(id: string): Promise<void> {
  await tauriInvoke("suggest_chapters", { id });
}

/** Ask the LLM to extract action items / tasks for a recording now (on demand).
 *  The structured tasks land on the recording (preserving any `done` flag on a
 *  surviving task); a `tasks_updated` event fires when ready. Task counterpart of
 *  {@link suggestEntities}. */
export async function suggestTasks(id: string): Promise<void> {
  await tauriInvoke("suggest_tasks", { id });
}

/** Toggle (or set) one task's `done` flag. Emits `tasks_updated` for the
 *  recording so open views refresh the chips. The one task mutation (entities
 *  have no analogue). `taskId` is the task row id from {@link Task}. */
export async function setTaskDone(id: string, taskId: number, done: boolean): Promise<void> {
  await tauriInvoke("set_task_done", { id, taskId, done });
}

/** Add a user-created task to a recording. Manual tasks survive re-extraction. */
export async function addTask(id: string, text: string, dueHint?: string | null): Promise<void> {
  await tauriInvoke("add_task", { id, text, dueHint: dueHint ?? null });
}

/** Edit one task's text (and optional free-text due hint). */
export async function updateTask(
  id: string,
  taskId: number,
  text: string,
  dueHint?: string | null,
): Promise<void> {
  await tauriInvoke("update_task", { id, taskId, text, dueHint: dueHint ?? null });
}

/** Delete one task from a recording. */
export async function deleteTask(id: string, taskId: number): Promise<void> {
  await tauriInvoke("delete_task", { id, taskId });
}

/** Set the user's task order for a recording (drag-reorder). */
export async function reorderTasks(id: string, taskIds: number[]): Promise<void> {
  await tauriInvoke("reorder_tasks", { id, taskIds });
}

/** Add a user-curated entity to a recording (survives re-extraction). */
export async function addEntity(id: string, kind: string, value: string): Promise<void> {
  await tauriInvoke("add_entity", { id, kind, value });
}

/** Edit one entity in place (fix its kind/value), keyed by its current pair. */
export async function updateEntity(
  id: string,
  kind: string,
  value: string,
  newKind: string,
  newValue: string,
): Promise<void> {
  await tauriInvoke("update_entity", { id, kind, value, newKind, newValue });
}

/** Delete one entity from a recording, keyed by (kind, value). */
export async function deleteEntity(id: string, kind: string, value: string): Promise<void> {
  await tauriInvoke("delete_entity", { id, kind, value });
}

/** Library-wide merge: fold variant values of a kind into a canonical value. */
export async function mergeEntities(
  kind: string,
  fromValues: string[],
  toValue: string,
): Promise<void> {
  await tauriInvoke("merge_entities", { kind, fromValues, toValue });
}

/** Approve one suggested tag: creates the tag if needed, attaches it, and
 *  removes it from the suggestion list. Returns the (created) tag. */
export async function approveTagSuggestion(id: string, name: string): Promise<Tag> {
  return await tauriInvoke<Tag>("approve_tag_suggestion", { id, name });
}

/** Drop every pending tag suggestion across the whole library. Returns how
 *  many recordings had suggestions to clear. */
export async function clearAllTagSuggestions(): Promise<number> {
  const res = await tauriInvoke<{ cleared: number }>("clear_all_tag_suggestions");
  return res?.cleared ?? 0;
}

/** Dismiss one suggested tag (drops it from the suggestion list). */
export async function dismissTagSuggestion(id: string, name: string): Promise<void> {
  await tauriInvoke("dismiss_tag_suggestion", { id, name });
}

/**
 * Set (or clear) the custom display name for one diarized speaker label of a
 * recording. `speakerLabel` is the 1-based `[Speaker N]` index; pass an empty
 * `name` to clear the mapping (reverts to "Speaker N"). The stored transcript
 * is never rewritten — names are applied at display/export time. Re-fetch the
 * recording (or listen for `SpeakerNameUpdated`) to pick up the new map.
 */
export async function setSpeakerName(
  id: string,
  speakerLabel: number,
  name: string,
): Promise<void> {
  await tauriInvoke("set_speaker_name", { id, speakerLabel, name });
}

// ── In-recording speaker correction (U1) ───────────────────────────────────────
// Unlike setSpeakerName (display-only), these change which segment belongs to
// which speaker. The daemon keeps `transcript_segments` authoritative and rebuilds
// the prose `[Speaker N]:` markers in one transaction, then emits
// `SpeakerNameUpdated` — so re-fetch the recording / segments (or let the event
// reload) to pick up the new attribution. Labels are 1-based `[Speaker N]`
// indices; `idx`/`segmentIdxs` are the 0-based segment indices from
// {@link getSegments} (the raw timeline).

/** Reassign one transcript segment to a different speaker label. `idx` is the
 *  0-based segment index from {@link getSegments}; `newLabel` is the 1-based
 *  `[Speaker N]` index — a brand-new label simply starts existing (no name or
 *  voiceprint is created for it). Rejects when no segment has that `idx`. */
export async function reassignSegmentSpeaker(
  id: string,
  idx: number,
  newLabel: number,
): Promise<void> {
  await tauriInvoke("reassign_segment_speaker", { id, idx, newLabel });
}

/** Merge two of a recording's speakers: every `fromLabel` segment becomes
 *  `intoLabel`, then `fromLabel` ceases to exist. `into` keeps its name (adopts
 *  `from`'s only when unnamed); `from`'s voiceprint is dropped. Rejects when no
 *  segment carries `fromLabel`, or when the labels are equal / below 1. */
export async function mergeSpeakers(
  id: string,
  fromLabel: number,
  intoLabel: number,
): Promise<void> {
  await tauriInvoke("merge_speakers", { id, fromLabel, intoLabel });
}

/** Split some of a speaker's segments off onto a fresh label. The listed
 *  `segmentIdxs` (0-based, from {@link getSegments}) move from `label` to
 *  `newLabel` (which starts with no name/voiceprint); every other segment of
 *  `label` stays. Rejects when any idx is missing or doesn't currently carry
 *  `label`, when the list is empty, or when the labels are equal / below 1. */
export async function splitSpeaker(
  id: string,
  label: number,
  segmentIdxs: number[],
  newLabel: number,
): Promise<void> {
  await tauriInvoke("split_speaker", { id, label, segmentIdxs, newLabel });
}

// ── Named-speaker recognition (#9) ─────────────────────────────────────────────

/** A recognized-speaker suggestion: an unnamed diarized speaker whose voiceprint
 *  matched a known voice. */
export type SpeakerSuggestion = {
  speaker_label: number;
  name: string;
  named_voice_id: string;
  score: number;
};

/** A named voice in the cross-recording library. */
export type NamedVoice = { id: string; name: string; samples: number };

/** On-demand recognition for a recording: unnamed speakers matching a known
 *  voice. Empty when recognition is off or nothing matches. */
export async function recognizeSpeakers(id: string): Promise<SpeakerSuggestion[]> {
  return await tauriInvoke<SpeakerSuggestion[]>("recognize_speakers", { id });
}

/** Dismiss a recognized-speaker suggestion so it isn't offered again. */
export async function dismissSpeakerSuggestion(
  id: string,
  speakerLabel: number,
): Promise<void> {
  await tauriInvoke("dismiss_speaker_suggestion", { id, speakerLabel });
}

/** The named-voice library (Speaker Library manager). */
export async function listNamedVoices(): Promise<NamedVoice[]> {
  return await tauriInvoke<NamedVoice[]>("list_named_voices");
}

/** Rename a named voice. */
export async function renameNamedVoice(id: string, name: string): Promise<void> {
  await tauriInvoke("rename_named_voice", { id, name });
}

/** Merge one named voice into another; resolves to whether a merge happened. */
export async function mergeNamedVoices(fromId: string, intoId: string): Promise<boolean> {
  const r = await tauriInvoke<{ merged: boolean }>("merge_named_voices", { fromId, intoId });
  return r.merged;
}

/** Forget a named voice; resolves to whether an entry was removed. */
export async function forgetNamedVoice(id: string): Promise<boolean> {
  const r = await tauriInvoke<{ removed: boolean }>("forget_named_voice", { id });
  return r.removed;
}

// ── Tags ─────────────────────────────────────────────────────────────────────
// Every mutation below also broadcasts a `tag_*` daemon event, so tag surfaces
// (sidebar, chips, Tag Manager) refresh themselves without explicit wiring.

/** A catalog tag. `color` is a `#rrggbb` hex string or null (theme accent). */
export type Tag = { id: number; name: string; color: string | null };

/** Tags attached to at least one recording (what the sidebar lists). */
export async function listTags(): Promise<Tag[]> {
  return await tauriInvoke<Tag[]>("list_tags");
}

/** Returns every tag, orphaned ones included — used by the Tag Manager. */
export async function listAllTags(): Promise<Tag[]> {
  return await tauriInvoke<Tag[]>("list_all_tags");
}

/** Create a tag (name must be unique; rejects on a duplicate). Returns it. */
export async function addTag(name: string, color?: string): Promise<Tag> {
  return await tauriInvoke<Tag>("add_tag", { name, color: color ?? null });
}

/** Rename / recolor a tag. The change shows everywhere it's attached. */
export async function updateTag(id: number, name: string, color?: string | null): Promise<Tag> {
  return await tauriInvoke<Tag>("update_tag", { id, name, color: color ?? null });
}

/** Delete a tag everywhere — it detaches from every recording it was on. */
export async function deleteTag(id: number): Promise<void> {
  await tauriInvoke("delete_tag", { id });
}

/** Attach an existing tag to a recording (idempotent). */
export async function attachTag(recordingId: string, tagId: number): Promise<void> {
  await tauriInvoke("attach_tag", { recordingId, tagId });
}

/** Detach a tag from a recording (the tag itself survives, even unused). */
export async function detachTag(recordingId: string, tagId: number): Promise<void> {
  await tauriInvoke("detach_tag", { recordingId, tagId });
}

/** The tags attached to one recording. */
export async function tagsFor(recordingId: string): Promise<Tag[]> {
  return await tauriInvoke<Tag[]>("tags_for", { recordingId });
}

/**
 * Map of tag id → number of recordings it's attached to. Tags with no
 * attachments are absent from the map (treat as 0). Powers the Tag Manager's
 * usage counts. Keys arrive as strings (JSON object keys).
 */
export async function tagUsageCounts(): Promise<Record<string, number>> {
  return await tauriInvoke<Record<string, number>>("tag_usage_counts");
}

/** Recording counts per Library type-filter, computed in SQL in one pass. Powers
 *  the sidebar's Library count badges (mirrors the tag usage counts). */
export type KindCounts = {
  all: number;
  single: number;
  meeting: number;
  in_place: number;
  favorite: number;
  /** Pinned recordings (the sidebar "Pinned" badge). */
  pinned: number;
  /** Distinct recordings carrying at least one tag (the sidebar "Tagged" badge). */
  tagged: number;
  /** Recordings carrying no tags (the sidebar "Untagged" badge). */
  untagged: number;
};

/** Fetch the per-kind recording counts for the Library section badges. */
export async function kindCounts(): Promise<KindCounts> {
  return await tauriInvoke<KindCounts>("kind_counts");
}

// ── Entities (cross-recording facet) ──────────────────────────────────────────

/** One row of the cross-recording entity facet: a distinct extracted entity
 *  (`kind` = person/org/topic/term, `value` = surface text) plus how many
 *  recordings mention it. The entity counterpart of a `Tag` + its usage count. */
export type EntityFacet = { kind: string; value: string; count: number };

/** Every distinct extracted entity across the library with its recording count,
 *  kind- then value-sorted. Powers the sidebar's browse-by-entity surface (the
 *  entity counterpart of {@link listAllTags} + {@link tagUsageCounts}). The
 *  entity *filter* itself rides on `listRecordings` via `ListFilter.entity_value`
 *  / `entity_kind`. */
export async function listAllEntities(): Promise<EntityFacet[]> {
  return await tauriInvoke<EntityFacet[]>("list_all_entities");
}

// ── Tasks (cross-recording list) ──────────────────────────────────────────────

/** One row of the cross-recording task list: a task plus enough to link back to
 *  its recording (`recording_id` + `title`). The task counterpart of an
 *  {@link EntityFacet}, but per-task rather than per distinct value. */
export type TaskWithRecording = {
  recording_id: string;
  title?: string | null;
  id: number;
  text: string;
  due_hint?: string | null;
  done: boolean;
};

/** Every extracted task across the library — the "everything I have to do" list —
 *  open first then newest recording first, each carrying its `recording_id` +
 *  `title`. When `onlyOpen` is set, done tasks are dropped. Powers the sidebar's
 *  Tasks section. The task counterpart of {@link listAllEntities}; the
 *  per-recording task *filter* rides on `listRecordings` via
 *  `ListFilter.task_state`. */
export async function listAllTasks(onlyOpen = false): Promise<TaskWithRecording[]> {
  return await tauriInvoke<TaskWithRecording[]>("list_all_tasks", { onlyOpen });
}

/** Library-wide task counts (`{ open, total }`). The cheap badge read the sidebar
 *  Tasks section uses instead of fetching every row via {@link listAllTasks} just
 *  to count it. */
export type TaskCounts = { open: number; total: number };

/** Fetch the library-wide open/total task counts for the sidebar Tasks badges. */
export async function taskCounts(): Promise<TaskCounts> {
  return await tauriInvoke<TaskCounts>("task_counts");
}

/**
 * Merge one tag into another: every recording tagged `fromId` is re-tagged
 * `intoId` (de-duplicated), then `fromId` is deleted. A no-op if equal.
 */
export async function mergeTags(fromId: number, intoId: number): Promise<void> {
  await tauriInvoke("merge_tags", { fromId, intoId });
}

// ── Config profiles ─────────────────────────────────────────────────────────

/** List the names of all saved config profiles. */
export async function listProfiles(): Promise<string[]> {
  return await tauriInvoke<string[]>("list_profiles");
}

/** A saved profile with metadata, for the Profile Manager. */
export type ProfileInfo = {
  name: string;
  /** Last-modified time in ms since the Unix epoch, or null if unreadable. */
  modified_ms: number | null;
};

/** List saved profiles with their last-modified time. */
export async function listProfilesDetailed(): Promise<ProfileInfo[]> {
  return await tauriInvoke<ProfileInfo[]>("list_profiles_detailed");
}

/** Rename a saved profile. Fails if the source is missing or the target exists. */
export async function renameProfile(from: string, to: string): Promise<void> {
  await tauriInvoke("rename_profile", { from, to });
}

/** Snapshot the current config.toml under the given profile name. */
export async function saveProfile(name: string): Promise<void> {
  await tauriInvoke("save_profile", { name });
}

/** Switch the active config to the named profile (and reload the daemon). */
export async function switchProfile(name: string): Promise<void> {
  await tauriInvoke("switch_profile", { name });
}

/** Delete a saved profile (does not touch the live config). */
export async function deleteProfile(name: string): Promise<void> {
  await tauriInvoke("delete_profile", { name });
}
