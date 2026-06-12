/** Shared formatting utilities used across RecordingsList, RecordingDetail, and TagChips. */

/** A timeline offset in milliseconds as a clock position: `m:ss`, or
 *  `h:mm:ss` past an hour. Used by the timeline views' time gutters (distinct
 *  from `formatDuration`, which renders lengths like "5m03s"). */
export function fmtClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}

/** Format a duration in milliseconds as a human-readable string. */
export function formatDuration(ms: number): string {
  // Under a minute: precise seconds (e.g. "46.2s").
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const totalSec = Math.floor(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  // An hour or more: "1h05m"; otherwise "5m03s".
  if (h > 0) return `${h}h${m.toString().padStart(2, "0")}m`;
  return `${m}m${s.toString().padStart(2, "0")}s`;
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
 * Every pipeline step has its own label (Transcribing, Cleaning Up,
 * Summarizing, Tagging, Hook Running) so the list/detail/activity views show
 * exactly which step a recording is on, never a generic "Pending".
 */
export function statusLabel(status: string): string {
  switch (status) {
    case "done":              return "Done";
    case "transcribe_failed": return "Transcription Failed";
    case "hook_failed":       return "Hook Failed";
    case "recording":         return "Recording";
    case "paused":            return "Paused";
    case "transcribing":      return "Transcribing";
    case "cleaning_up":       return "Cleaning Up";
    case "summarizing":       return "Summarizing";
    case "tagging":           return "Tagging";
    case "hook_running":      return "Hook Running";
    default:                  return status;
  }
}

/** Escape HTML special characters for safe innerHTML insertion. */
export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/**
 * Escape a string for safe insertion into a double-quoted HTML attribute.
 * Extends escapeHtml by also encoding `"`, so the value cannot break out of
 * the attribute. Use for `value="..."`, `style="..."`, `title="..."`, etc.
 */
export function escapeAttr(s: string): string {
  return escapeHtml(s).replace(/"/g, "&quot;");
}

/**
 * Returns HTML with occurrences of `term` inside `text` wrapped in
 * `<mark class="search-hit">` tags. Both the surrounding text and the
 * matched portions are HTML-escaped so it is safe to inject into innerHTML.
 * Returns plain-escaped text when term is empty.
 */
export function highlightMatch(text: string, term: string): string {
  if (!term) return escapeHtml(text);
  const escaped = term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(escaped, "gi");
  const parts: string[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    parts.push(escapeHtml(text.slice(lastIndex, match.index)));
    parts.push(`<mark class="search-hit">${escapeHtml(match[0])}</mark>`);
    lastIndex = re.lastIndex;
    if (re.lastIndex === match.index) re.lastIndex++;
  }
  parts.push(escapeHtml(text.slice(lastIndex)));
  return parts.join("");
}

/**
 * Human-readable word count + reading time for a transcript, e.g.
 * `"243 words · ~1 min read"`. Returns `""` for empty/whitespace text (so the
 * caller can omit the element entirely). Reading time assumes ~200 wpm, min 1.
 */
export function wordCountSummary(text: string): string {
  const trimmed = text.trim();
  if (!trimmed) return "";
  const words = trimmed.split(/\s+/).length;
  const minutes = Math.max(1, Math.round(words / 200));
  const wordLabel = words === 1 ? "word" : "words";
  const minLabel = minutes === 1 ? "min" : "mins";
  return `${words} ${wordLabel} · ~${minutes} ${minLabel} read`;
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
