/**
 * Date-bucketing for the recordings list's day groups. Kept separate from
 * utils/format.ts because it's about CALENDAR grouping, not value formatting.
 */

/**
 * The day-group label for a timestamp, relative to today: "Today",
 * "Yesterday", "Last 7 Days", "Last 30 Days", or "Older". The recordings
 * list renders one section header per distinct label, so the buckets must
 * stay ordered (newest first) and mutually exclusive. Future timestamps
 * (clock skew) land in "Today" rather than getting a bucket of their own.
 */
export function formatDay(iso: string): string {
  const d = new Date(iso);
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const diffTime = today.getTime() - d.getTime();
  const diffDays = Math.ceil(diffTime / (1000 * 60 * 60 * 24));

  if (diffDays <= 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  if (diffDays <= 7) return "Last 7 Days";
  if (diffDays <= 30) return "Last 30 Days";
  return "Older";
}

/**
 * The Day column's calendar date for a timestamp — a compact zero-padded
 * `MM/DD` (month first, the default) or `DD/MM` when `dayFirst` is set
 * (`interface.date_day_first`). No year, to stay narrow. Replaces the relative
 * "Today / Yesterday" bucket label as the Day column's per-row value.
 */
export function formatDayDate(iso: string, dayFirst = false): string {
  const d = new Date(iso);
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return dayFirst ? `${dd}/${mm}` : `${mm}/${dd}`;
}
