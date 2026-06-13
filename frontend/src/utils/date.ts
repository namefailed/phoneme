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
