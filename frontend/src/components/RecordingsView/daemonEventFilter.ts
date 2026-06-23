// Which daemon events the home view reacts to — the pure name-classification
// behind subscribeToEvents. RecordingsView owns the subscription and decides what
// to do; these just answer "is this one I care about?" so the wiring stays a thin
// switch. See services/events.ts for the full event catalog.

/** A whole-meeting digest result (keyed by meeting_id, not a recording id): the
 *  merged view re-fetches itself when it's the meeting on screen. */
export function isMeetingDigestEvent(eventName: string): boolean {
  return eventName === "meeting_digest_updated" || eventName === "meeting_digest_failed";
}

/** A change that can alter the recordings list or the open detail (lifecycle,
 *  pipeline progress, content/speaker/entity/tag mutations) — funnels through
 *  refresh() so the list + open detail track it live without polling. */
export function isListRefreshEvent(eventName: string): boolean {
  switch (eventName) {
    case "recording_stopped":
    case "transcription_done":
    case "transcription_failed":
    // Each pipeline step writes its own status (Transcribing → Cleaning Up
    // → Summarizing → …) — refresh so the Status column tracks it live.
    case "pipeline_stage_changed":
    case "hook_done":
    case "hook_failed":
    case "recording_deleted":
    case "transcript_updated":
    case "summary_updated":
    // Entity extraction landed — refresh so the detail provenance line
    // (entities_model) and any list signal update live.
    case "entities_updated":
    case "speaker_name_updated":
    // Tag mutations change the Tags column — refresh so it updates live
    // instead of needing a manual reload.
    case "tag_attached":
    case "all_tag_suggestions_cleared":
    case "tag_detached":
    case "tag_updated":
    case "tag_deleted":
    case "tag_created":
      return true;
    default:
      return false;
  }
}
