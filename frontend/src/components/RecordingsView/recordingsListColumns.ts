// Pure column-presentation data + geometry for the recordings table: the header
// labels, the per-column default widths, the px parser the grid math leans on,
// and the error-text fallback for a row with no transcript. Stateless lookups
// lifted out of RecordingsList.ts; the grid-template assembly stays in the
// component because it threads through instance state (current widths / config).

import type { Recording } from "../../services/ipc";

/** Header label per column key — emoji for the quick-affordance columns
 *  (pin/star), worded labels for the data columns. Unknown keys fall back to the
 *  raw key at the call site. */
export const COL_LABELS: Record<string, string> = {
  pinned: "📌",
  favorite: "⭐",
  day: "Day",
  time: "Time",
  duration: "Duration",
  status: "Status",
  title: "Title",
  tags: "Tags",
  model: "Transcript Model",
  cleanup_model: "Post-Process Model",
  summary_model: "Summary Model",
  title_model: "Title Model",
  tag_model: "Auto-Tag Model",
  diarization_model: "Diarization Model",
  diarized: "Diarized",
  user_edited: "Edited",
  source: "Source",
  transcript: "Transcript",
};

// Sensible per-column defaults sized to what each typically holds: a star, a
// date/time, a short duration, a status pill (up to "Hook Running"), a title, a
// couple of tag chips, model names (e.g. whisper-large-v3-turbo), boolean badges,
// a source label. Transcript takes the rest (1fr).
export const COL_DEFAULT_WIDTHS: Record<string, string> = {
  pinned: "40px",
  favorite: "40px",
  day: "96px",
  time: "96px",
  duration: "88px",
  status: "128px",
  title: "220px",
  tags: "140px",
  model: "150px",
  cleanup_model: "150px",
  summary_model: "150px",
  title_model: "150px",
  tag_model: "150px",
  diarization_model: "150px",
  diarized: "64px",
  user_edited: "72px",
  source: "84px",
  transcript: "1fr",
};

/** Parse a `"123px"` width into a number, 0 when it isn't a px value. Used by the
 *  grid min-width / template math. */
export function parsePx(w: string): number {
  const m = /([\d.]+)px/.exec(w);
  return m ? parseFloat(m[1]) : 0;
}

/** Turn the visible columns + their resolved widths into the row's CSS grid
 *  geometry: the `grid-template-columns` string and the row min-width. Pure — no
 *  component state — so the render method just consumes the result.
 *
 *  The transcript "read more by scrolling" behavior applies only when transcript
 *  is the last column: there it sizes to its content (`max-content`, capped at
 *  1200px via `.transcript-tail .rec-preview`) so the row grows past the pane and
 *  you scroll to read more. Anywhere else (rearranged in Appearance settings) it's
 *  a normal, resizable, fixed-width column, never ballooning mid-row. A cell-less
 *  `minmax(0,1fr)` filler is appended only when no column is already flexible, so
 *  the row always fills the pane to the splitter. The min-width (used only when
 *  transcript isn't the tail) lets a row's background/selection extend the full
 *  scrolled width when the fixed columns overflow the pane. */
export function buildGridGeometry(
  visibleCols: string[],
  activeWidths: string[],
): { transcriptIsLast: boolean; gridTemplate: string; gridMinWidth: number } {
  const checkboxColWidth = "28px";
  const transcriptIsLast = visibleCols[visibleCols.length - 1] === "transcript";
  const widthsForGrid = activeWidths.map((w, i) => {
    if (visibleCols[i] !== "transcript") return w;
    const px = w.trim().endsWith("px") ? w.trim() : null;
    if (transcriptIsLast) return `minmax(${px ?? "160px"}, max-content)`;
    return px ?? "300px"; // not last → a normal, resizable fixed-width column
  });
  const hasFlexTrack = widthsForGrid.some((t) => t.includes("fr"));
  const gridTemplate = [
    checkboxColWidth,
    ...widthsForGrid,
    ...(hasFlexTrack ? [] : ["minmax(0, 1fr)"]),
  ].join(" ");
  const gridMinWidth =
    28 +
    activeWidths.reduce((sum, w, i) => {
      const px = parsePx(w);
      if (visibleCols[i] === "transcript") return sum + (px || (transcriptIsLast ? 160 : 300));
      return sum + (px || 120);
    }, 0);
  return { transcriptIsLast, gridTemplate, gridMinWidth };
}

/** Placeholder transcript text for a row that has none — the failure reason when
 *  there is one, else a status-appropriate note. */
export function truncatedError(r: Recording): string {
  if (r.error_message) return `(${r.error_message})`;
  if (r.status === "transcribe_failed") return "(transcription failed)";
  if (r.status === "hook_failed") return "(hook failed)";
  if (r.status === "cancelled") return "(cancelled)";
  return "(processing…)";
}
