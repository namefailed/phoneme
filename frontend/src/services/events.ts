import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type DaemonEvent =
  | { event: "recording_started"; id: string; started_at: string }
  | { event: "recording_stopped"; id: string; duration_ms: number; audio_path: string }
  | { event: "transcription_started"; id: string }
  | { event: "transcription_done"; id: string; transcript: string }
  | { event: "transcription_failed"; id: string; error: string }
  | { event: "hook_started"; id: string }
  | { event: "hook_done"; id: string; exit_code: number }
  | { event: "hook_failed"; id: string; error: string }
  | { event: "queue_depth_changed"; pending: number; processing: number; failed: number }
  | { event: "whisper_status_changed"; reachable: boolean }
  | { event: "recording_deleted"; id: string }
  | { event: "transcript_updated"; id: string }
  | { event: "tag_created"; id: number }
  | { event: "tag_deleted"; id: number }
  | { event: "tag_attached"; tag_id: number }
  | { event: "tag_detached"; tag_id: number };

export type EventHandler = (event: DaemonEvent) => void;

export async function subscribe(handler: EventHandler): Promise<UnlistenFn> {
  return await listen<DaemonEvent>("daemon-event", (e) => handler(e.payload));
}

export async function onMenu(name: string, handler: () => void): Promise<UnlistenFn> {
  return await listen(`menu:${name}`, () => handler());
}

export async function onNav(name: string, handler: () => void): Promise<UnlistenFn> {
  return await listen(`nav:${name}`, () => handler());
}
