// Which daemon events the home view reacts to — the pure name-classification
// behind subscribeToEvents. RecordingsView owns the subscription and decides what
// to do; these just answer "is this one I care about?" so the wiring stays a thin
// switch. See services/events.ts for the full event catalog.

/** A whole-meeting digest result (keyed by meeting_id, not a recording id): the
 *  merged view re-fetches itself when it's the meeting on screen. */
export function isMeetingDigestEvent(eventName: string): boolean {
  return eventName === "meeting_digest_updated" || eventName === "meeting_digest_failed";
}

/** Events that can alter the recordings list or the open detail (lifecycle,
 *  pipeline progress, content/speaker/entity/tag mutations). */
const LIST_REFRESH_EVENTS = new Set([
  "recording_stopped",
  "transcription_done",
  "transcription_failed",
  // Each pipeline step writes its own status (Transcribing → Cleaning Up
  // → Summarizing → …) — refresh so the Status column tracks it live.
  "pipeline_stage_changed",
  "hook_done",
  "hook_failed",
  "recording_deleted",
  "transcript_updated",
  "summary_updated",
  // Entity extraction landed — refresh so the detail provenance line
  // (entities_model) and any list signal update live.
  "entities_updated",
  "speaker_name_updated",
  // Tag mutations change the Tags column — refresh so it updates live
  // instead of needing a manual reload.
  "tag_attached",
  "all_tag_suggestions_cleared",
  "meeting_name_updated",
  "tag_detached",
  "tag_updated",
  "tag_deleted",
  "tag_created",
]);

/** Whether an event funnels through refresh() so the list + open detail track it
 *  live without polling. */
export function isListRefreshEvent(eventName: string): boolean {
  return LIST_REFRESH_EVENTS.has(eventName);
}
