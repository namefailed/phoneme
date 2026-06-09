/**
 * This module provides the frontend TypeScript boundary to the Tauri Rust backend.
 * It encapsulates the `invoke` calls into strictly typed async functions.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";

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
  /** LLM-generated summary of the transcript, if one has been produced. */
  summary?: string | null;
  /** The LLM model used to produce `summary`, if any. */
  summary_model?: string | null;
  /** Tags associated with this recording */
  tags?: Array<{ id: number; name: string; color?: string | null }>;
};

export type RecordMode = "hold" | "oneshot" | `duration:${number}`;

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
};

/**
 * Fetches a list of recordings matching the given filter.
 * The results are paginated or limited by the backend (default limit 50).
 */
export async function listRecordings(filter: ListFilter = {}): Promise<Recording[]> {
  return await tauriInvoke<Recording[]>("list_recordings", { filter });
}

export async function getRecording(id: string): Promise<Recording> {
  return await tauriInvoke<Recording>("get_recording", { id });
}

export interface SemanticSearchResult {
  recording: Recording;
  score: number;
}

export async function semanticSearch(query: string, limit: number = 20): Promise<SemanticSearchResult[]> {
  return await tauriInvoke<SemanticSearchResult[]>("semantic_search", { query, limit });
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

export async function recordStop(): Promise<void> {
  await tauriInvoke("record_stop");
}

export async function recordPause(): Promise<void> {
  await tauriInvoke("record_pause");
}

export async function recordResume(): Promise<void> {
  await tauriInvoke("record_resume");
}

export async function updateMeetingName(meetingId: string, name: string | null): Promise<void> {
  await tauriInvoke("update_meeting_name", { meetingId, name });
}

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
 * Import an existing audio file (wav/mp3/m4a). The daemon decodes it to a
 * canonical WAV and transcribes it like a normal recording. Returns the new id.
 */
export async function importRecording(path: string): Promise<{ id: string }> {
  return await tauriInvoke<{ id: string }>("import_recording", { path });
}

/** File extensions accepted by the import flow (no leading dot). */
export const IMPORT_AUDIO_EXTENSIONS = ["wav", "mp3", "m4a"] as const;

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

export async function daemonStatus(): Promise<{ running: boolean; pid: number }> {
  return await tauriInvoke("daemon_status");
}

export type Tag = { id: number; name: string; color: string | null };

export async function listTags(): Promise<Tag[]> {
  return await tauriInvoke<Tag[]>("list_tags");
}

/** Returns ALL tags including orphaned ones — used by the Tag Manager. */
export async function listAllTags(): Promise<Tag[]> {
  return await tauriInvoke<Tag[]>("list_all_tags");
}

export async function addTag(name: string, color?: string): Promise<Tag> {
  return await tauriInvoke<Tag>("add_tag", { name, color: color ?? null });
}

export async function updateTag(id: number, name: string, color?: string | null): Promise<Tag> {
  return await tauriInvoke<Tag>("update_tag", { id, name, color: color ?? null });
}

export async function deleteTag(id: number): Promise<void> {
  await tauriInvoke("delete_tag", { id });
}

export async function attachTag(recordingId: string, tagId: number): Promise<void> {
  await tauriInvoke("attach_tag", { recordingId, tagId });
}

export async function detachTag(recordingId: string, tagId: number): Promise<void> {
  await tauriInvoke("detach_tag", { recordingId, tagId });
}

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
