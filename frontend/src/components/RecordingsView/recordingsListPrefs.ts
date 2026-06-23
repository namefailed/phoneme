// Per-device display prefs for the recordings list, persisted in localStorage:
// which meeting groups are expanded, column widths (keyed by column name), and
// each meeting's cosmetic icon. All synced config lives elsewhere — these are
// the bits that are device-local on purpose (see the notes per section). Pure
// storage helpers, lifted out of RecordingsList.ts and imported back.

/** Which meeting groups are expanded — remembered across reloads (per device). */
const LS_EXPANDED_MEETINGS = "phoneme.expandedMeetings";
export function loadExpandedMeetings(): string[] {
  try {
    const raw = localStorage.getItem(LS_EXPANDED_MEETINGS);
    const arr = raw ? JSON.parse(raw) : [];
    return Array.isArray(arr) ? arr.filter((s): s is string => typeof s === "string") : [];
  } catch {
    return [];
  }
}
export function saveExpandedMeetings(set: Set<string>): void {
  try {
    localStorage.setItem(LS_EXPANDED_MEETINGS, JSON.stringify([...set]));
  } catch {
    /* private mode / quota — non-fatal */
  }
}

/** Column widths, keyed by column name (per device). Stored here rather than in
 *  the synced config: the config array is positional, so it resets whenever a
 *  column is added, removed, or reordered. A name-keyed map survives all three. */
const LS_COL_WIDTHS = "phoneme.recordings.colWidths";
export function loadColWidths(): Record<string, string> {
  try {
    const raw = localStorage.getItem(LS_COL_WIDTHS);
    const obj = raw ? JSON.parse(raw) : {};
    return obj && typeof obj === "object" && !Array.isArray(obj) ? obj : {};
  } catch {
    return {};
  }
}
export function saveColWidths(map: Record<string, string>): void {
  try {
    localStorage.setItem(LS_COL_WIDTHS, JSON.stringify(map));
  } catch {
    /* private mode / quota — non-fatal */
  }
}

/** Per-meeting display icon (a cosmetic per-device pref, like the meeting name
 *  is in the catalog). Keyed by meeting id. */
const LS_MEETING_ICONS = "phoneme.meetingIcons";
export const DEFAULT_MEETING_ICON = "👥";
/** Emoji choices offered in the meeting icon picker. */
export const MEETING_ICON_CHOICES = [
  "👥", "🎙️", "📞", "💼", "🧑‍🏫", "🎧", "🗣️", "📅", "🤝", "🎬", "📋", "💡",
  "📝", "🧠", "⭐", "🔥", "🎯", "🚀", "🐞", "🔧", "💬", "📣", "🎓", "🩺",
];
function loadMeetingIcons(): Record<string, string> {
  try {
    const raw = localStorage.getItem(LS_MEETING_ICONS);
    const obj = raw ? JSON.parse(raw) : {};
    return obj && typeof obj === "object" ? obj : {};
  } catch {
    return {};
  }
}
export function meetingIcon(meetingId: string): string {
  return loadMeetingIcons()[meetingId] || DEFAULT_MEETING_ICON;
}
export function saveMeetingIcon(meetingId: string, icon: string): void {
  try {
    const all = loadMeetingIcons();
    all[meetingId] = icon;
    localStorage.setItem(LS_MEETING_ICONS, JSON.stringify(all));
  } catch {
    /* private mode — non-fatal */
  }
}
