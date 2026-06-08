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

export async function retranscribeRecording(id: string, model: string | null = null, runHooks: boolean | null = null): Promise<void> {
  await tauriInvoke("retranscribe_recording", { id, model, runHooks });
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

// ── Config profiles ─────────────────────────────────────────────────────────

/** List the names of all saved config profiles. */
export async function listProfiles(): Promise<string[]> {
  return await tauriInvoke<string[]>("list_profiles");
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
