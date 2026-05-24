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
};

export type RecordMode = "hold" | "oneshot" | `duration:${number}`;

export type ListFilter = {
  limit?: number | null;
  since?: string | null;
  status?: string | null;
  search?: string | null;
  tag_id?: number | null;
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

export async function recordCancel(): Promise<void> {
  await tauriInvoke("record_cancel");
}

export async function replayRecording(id: string): Promise<void> {
  await tauriInvoke("replay_recording", { id });
}

export async function refireHook(id: string): Promise<void> {
  await tauriInvoke("refire_hook", { id });
}

/**
 * Manually update the text transcript of a specific recording.
 */
export async function updateTranscript(id: string, text: string): Promise<void> {
  await tauriInvoke("update_transcript", { id, text });
}

export async function daemonStatus(): Promise<{ running: boolean; pid: number }> {
  return await tauriInvoke("daemon_status");
}

export type Tag = { id: number; name: string; color: string | null };

export async function listTags(): Promise<Tag[]> {
  return await tauriInvoke<Tag[]>("list_tags");
}

export async function addTag(name: string, color?: string): Promise<Tag> {
  return await tauriInvoke<Tag>("add_tag", { name, color: color ?? null });
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
