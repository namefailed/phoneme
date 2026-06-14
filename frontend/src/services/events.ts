/**
 * The daemon event stream — how the UI learns that anything changed. The
 * daemon broadcasts every state change over IPC; the tray's bridge re-emits
 * each one as the Tauri event `"daemon-event"`, and this module types that
 * stream and hands out subscriptions. The whole UI is event-driven off it:
 * views re-fetch on the events they care about rather than polling.
 *
 * Wire contract: `DaemonEvent` mirrors the daemon's `Event` enum in
 * `crates/phoneme-ipc/src/schema.rs` (serde-tagged by the `event` field,
 * snake_case). New daemon events must be added to BOTH places — an unknown
 * event still arrives here, but no handler will match it.
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** A pipeline processing stage (mirrors the daemon's `PipelineStage`). */
export type PipelineStage =
  | "transcribing"
  | "cleaning_up"
  | "summarizing"
  | "tagging"
  | "running_hook"
  | "done"
  | "failed";

/** Human-readable label for a live pipeline stage. */
export function stageLabel(stage: PipelineStage): string {
  switch (stage) {
    case "transcribing": return "Transcribing…";
    case "cleaning_up": return "Cleaning up…";
    case "summarizing": return "Summarizing…";
    case "tagging": return "Suggesting tags…";
    case "running_hook": return "Running hook…";
    case "done": return "Done";
    case "failed": return "Failed";
  }
}

/**
 * One daemon broadcast, discriminated by `event`. Highlights:
 *
 *  - Recording lifecycle: `recording_started` / `_stopped` / `_paused` /
 *    `_resumed` / `_cancelled` / `_deleted`.
 *  - Pipeline: `transcription_started` → `transcription_partial` (live
 *    preview text) → `transcription_done` | `transcription_failed`, with
 *    `pipeline_stage_changed` marking each stage START (cleanup, summary,
 *    tagging, hook) and `llm_activity` streaming prompt/response deltas for
 *    the AI-activity log.
 *  - Content updates (`transcript_updated`, `summary_updated`/`_failed`,
 *    `notes_updated`, `speaker_name_updated`, `tag_suggestions_updated`):
 *    carry only the id — listeners re-fetch the recording for the new data.
 *  - Tags (`tag_*`): catalog-wide tag CRUD/attach changes; the sidebar and
 *    chip surfaces reload their tag lists on any of them.
 *  - Health/queue: `queue_depth_changed` (inbox counts), `whisper_status_changed`,
 *    `retention_warning`, and `preview_source_changed` (overlay track toggle).
 */
export type DaemonEvent =
  | { event: "recording_started"; id: string; started_at: string; meeting_id?: string | null; track?: string | null }
  | { event: "recording_stopped"; id: string; duration_ms: number; audio_path: string; meeting_id?: string | null }
  | { event: "transcription_started"; id: string }
  | { event: "transcription_partial"; id: string; text: string }
  | { event: "audio_level_sample"; id: string; level: number }
  | { event: "transcription_done"; id: string; transcript: string }
  | { event: "transcription_failed"; id: string; error: string }
  | { event: "pipeline_stage_changed"; id: string; stage: PipelineStage }
  | { event: "llm_activity"; id: string; stage: PipelineStage; prompt: string; delta: string; done: boolean }
  | { event: "hook_started"; id: string }
  | { event: "hook_done"; id: string; exit_code: number }
  | { event: "hook_failed"; id: string; error: string }
  | { event: "queue_depth_changed"; pending: number; processing: number; failed: number }
  | { event: "whisper_status_changed"; reachable: boolean }
  | { event: "recording_deleted"; id: string }
  | { event: "recording_cancelled"; id: string }
  | { event: "recording_paused"; id: string }
  | { event: "recording_resumed"; id: string }
  | { event: "retention_warning"; count: number; hours: number }
  | { event: "transcript_updated"; id: string }
  | { event: "summary_updated"; id: string }
  | { event: "summary_failed"; id: string; error: string }
  | { event: "notes_updated"; id: string }
  | { event: "speaker_name_updated"; id: string }
  | { event: "tag_created"; id: number }
  | { event: "tag_updated"; id: number }
  | { event: "tag_deleted"; id: number }
  | { event: "tag_attached"; tag_id: number }
  | { event: "tag_detached"; tag_id: number }
  | { event: "tag_suggestions_updated"; id: string }
  | { event: "all_tag_suggestions_cleared"; cleared: number }
  | { event: "preview_source_changed"; track: string };

/** Callback receiving every daemon event; switch on `event.event`. */
export type EventHandler = (event: DaemonEvent) => void;

/**
 * Subscribe `handler` to the full daemon event stream. Every subscriber gets
 * every event — filter inside the handler. Returns the unlisten function;
 * components MUST call it on teardown (`disconnectedCallback` / `dispose`) or
 * the handler outlives them. App-lifetime subscribers (the queue panel, step
 * notifications) deliberately never unlisten.
 */
export async function subscribe(handler: EventHandler): Promise<UnlistenFn> {
  return await listen<DaemonEvent>("daemon-event", (e) => handler(e.payload));
}

/** Listen for a tray-menu command (the tray emits `menu:<name>`, e.g.
 *  `menu:record` / `menu:stop`). Returns the unlisten function. */
export async function onMenu(name: string, handler: () => void): Promise<UnlistenFn> {
  return await listen(`menu:${name}`, () => handler());
}

/** Listen for a tray-menu navigation request (`nav:<name>`, e.g. `nav:settings`
 *  / `nav:doctor`). App routes these through its unsaved-edits guard. */
export async function onNav(name: string, handler: () => void): Promise<UnlistenFn> {
  return await listen(`nav:${name}`, () => handler());
}
