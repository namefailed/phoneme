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
 * Error behavior: every function REJECTS on failure with the structured
 * `{ kind, message }` command error (see utils/error.ts). Nothing here
 * toasts — callers decide how to surface failures.
 *
 * House rules for adding a call: argument keys are camelCase (Tauri converts
 * the top-level keys to the command's snake_case parameters — but NOT keys
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
  /** Whether the user hand-edited the transcript (independent of `model`). */
  user_edited?: boolean;
  /** Whether the user has starred this recording (the Favorites view). */
  favorite?: boolean;
  /** LLM-suggested tags awaiting approval (auto-tagging). Names only. */
  tag_suggestions?: string[];
  /** LLM-generated summary of the transcript, if one has been produced. */
  summary?: string | null;
  /** The LLM model used to produce `summary`, if any. */
  summary_model?: string | null;
  /** Display title — auto-generated (heuristic/LLM) or user-set. Null until
   *  generated; the UI falls back to the started-at timestamp. */
  title?: string | null;
  /** Whether `title` is auto-generated (true — the pipeline may refresh it)
   *  or user-set (false — auto writes never overwrite it). */
  title_is_auto?: boolean;
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

/** How a recording started via `recordStart` decides to stop: "hold" = on the
 *  explicit stop signal (Stop click / hotkey release), "oneshot" = by itself
 *  on silence (or the max-duration ceiling), `duration:N` = after exactly N
 *  seconds. See services/recordStopMode.ts for the UI-level mapping. */
export type RecordMode = "hold" | "oneshot" | `duration:${number}`;

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
export async function getSegments(id: string): Promise<TranscriptSegment[]> {
  return await tauriInvoke<TranscriptSegment[]>("get_segments", { id });
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
};

/** A recording's machine transcript words in timeline order. Like segments, an
 *  empty list is a normal state — older recordings predate word capture, and
 *  several providers return no per-word timing — so the synced-transcript view
 *  treats it as "no word timings" rather than an error. */
export async function getWords(id: string): Promise<TranscriptWord[]> {
  return await tauriInvoke<TranscriptWord[]>("get_words", { id });
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
 * Fetch all recordings belonging to a single meeting session (the two tracks
 * sharing a `meeting_id`), ordered by track then start time.
 */
export async function listSession(meetingId: string): Promise<Recording[]> {
  return await tauriInvoke<Recording[]>("list_meeting", { meetingId });
}

/**
 * Deletes a recording by ID. If keepAudio is true, the catalog entry is removed
 * but the raw `.wav` file is preserved on disk.
 */
export async function deleteRecording(id: string, keepAudio = false): Promise<void> {
  await tauriInvoke("delete_recording", { id, keepAudio });
}

/**
 * Initiates a new recording session. Returns the generated recording ID.
 */
export async function recordStart(mode: RecordMode): Promise<{ id: string }> {
  return await tauriInvoke<{ id: string }>("record_start", { mode });
}

/** Stop the active recording; it finalizes and enters the transcription
 *  queue (`recording_stopped` fires, then the pipeline events follow). */
export async function recordStop(): Promise<void> {
  await tauriInvoke("record_stop");
}

/** Pause the active recording's capture (a `recording_paused` event fires;
 *  the audio file simply stops growing until resume). */
export async function recordPause(): Promise<void> {
  await tauriInvoke("record_pause");
}

/** Resume a paused recording (`recording_resumed` fires). */
export async function recordResume(): Promise<void> {
  await tauriInvoke("record_resume");
}

/** Set (or clear, with `null`) the display name of a meeting session. Shown
 *  on the list's group header; the tracks themselves are untouched. */
export async function updateMeetingName(meetingId: string, name: string | null): Promise<void> {
  await tauriInvoke("update_meeting_name", { meetingId, name });
}

/** Abort the active recording and DISCARD its audio — nothing is transcribed
 *  and no catalog row survives (`recording_cancelled` fires). */
export async function recordCancel(): Promise<void> {
  await tauriInvoke("record_cancel");
}

/**
 * Meeting Mode (v1.6): start a dual-track recording. The daemon captures the
 * microphone AND the system audio (WASAPI loopback) concurrently as two
 * separate recordings linked by a shared `meeting_id`. Returns the session id.
 */
export async function startMeeting(): Promise<{ meeting_id: string }> {
  return await tauriInvoke<{ meeting_id: string }>("start_meeting");
}

/** Stop the active meeting. Both tracks are finalized and transcribed. */
export async function stopMeeting(): Promise<{ meeting_id: string }> {
  return await tauriInvoke<{ meeting_id: string }>("stop_meeting");
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
): Promise<void> {
  await tauriInvoke("retranscribe_recording", { id, model, runHooks, postProcess, allOverrides });
}

/**
 * Import an existing audio file (wav/mp3/m4a/flac). The daemon decodes it to a
 * canonical WAV and transcribes it like a normal recording. Returns the new id.
 */
export async function importRecording(path: string): Promise<{ id: string }> {
  return await tauriInvoke<{ id: string }>("import_recording", { path });
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

/**
 * Re-run ONLY the LLM post-processing ("cleanup") step on a recording's stored
 * transcript — without re-transcribing the audio. The preserved original
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
 * on demand, and store it. Reuses the configured `[llm_post_process]` provider
 * connection; `model`/`prompt` override the configured summary model/prompt for
 * this run only (never persisted). The summary text arrives via the
 * `SummaryUpdated` daemon event — re-fetch the recording when it fires.
 */
export async function rerunSummary(
  id: string,
  model: string | null = null,
  prompt: string | null = null,
): Promise<void> {
  await tauriInvoke("rerun_summary", { id, model, prompt });
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

/** Remove ALL still-pending items from the queue. Returns how many were removed. */
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

/** Whether the daemon process is running, and its pid. Answered by the TRAY
 *  (it owns the daemon process), so it works even when the daemon is down —
 *  the Doctor surfaces use it as the "is anything alive" check. */
export async function daemonStatus(): Promise<{ running: boolean; pid: number }> {
  return await tauriInvoke("daemon_status");
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

/** Returns ALL tags including orphaned ones — used by the Tag Manager. */
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
