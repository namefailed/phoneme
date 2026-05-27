/** Shared formatting utilities used across RecordingsList, RecordingDetail, and TagChips. */

/** Format a duration in milliseconds as a human-readable string. */
export function formatDuration(ms: number): string {
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60_000)}m${Math.floor((ms % 60_000) / 1000)
    .toString()
    .padStart(2, "0")}s`;
}

/**
 * Returns the CSS class for a recording status pill.
 * Maps the 6 backend statuses to the 3 visual states: done / failed / pending.
 */
export function statusToClass(status: string): "done" | "failed" | "pending" {
  if (status === "done") return "done";
  if (status === "transcribe_failed" || status === "hook_failed") return "failed";
  return "pending";
}

/**
 * Returns the full human-readable label for a recording status.
 * Preserves the distinction between Recording, Transcribing, and Hook Running
 * rather than collapsing all three into a generic "Pending".
 */
export function statusLabel(status: string): string {
  switch (status) {
    case "done":              return "Done";
    case "transcribe_failed": return "Transcribe Failed";
    case "hook_failed":       return "Hook Failed";
    case "recording":         return "Recording";
    case "transcribing":      return "Transcribing";
    case "hook_running":      return "Hook Running";
    default:                  return status;
  }
}

/** Escape HTML special characters for safe innerHTML insertion. */
export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Format a timestamp as a locale time string. */
export function formatTime(iso: string, use24h: boolean): string {
  const d = new Date(iso);
  return d.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    hour12: !use24h,
  });
}
