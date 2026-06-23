// Detail-pane 2D keyboard-grid geometry — the pure, stateless helpers behind the
// detail pane's vim navigation. RecordingsView owns the cursor state and the
// cell collection; this module just turns a flat cell list into visual rows and
// answers "which column sits nearest this x?" so j/k lands spatially.

/** One keyboard-navigable target in the detail pane's 2D grid. `button` clicks
 *  on Enter; `tags` focuses the add-tag input (Shift+Enter opens the Tag
 *  Manager); `editor` focuses the editable area inside its block (transcript /
 *  notes); `suggestion` is a whole AI tag-suggestion chip — Enter drops into a
 *  sub-step where h/l pick its ✓ (approve) / × (dismiss). */
export type DetailCell = { el: HTMLElement; kind: "button" | "tags" | "editor" | "waveform" | "suggestion" };

/** Group detail-pane cells into grid rows by their on-screen vertical position so
 *  navigation follows the visible layout — a button row that wraps at a narrow
 *  pane width becomes several grid rows automatically. Buckets by each cell's top
 *  edge (within a tolerance), not by vertical range overlap: a tall block like the
 *  transcript box needs to stay its own row while the buttons nested at its bottom
 *  (Speakers · Views · Versions) fall to the next row, and overlap-grouping would
 *  wrongly merge them. Within a row, cells are ordered left→right. */
export function bucketCellsByRow(cells: DetailCell[]): DetailCell[][] {
  const TOL = 10; // px; same-line cells share a top within this, a wrap exceeds it
  const withRects = cells
    .map((c) => ({ c, r: c.el.getBoundingClientRect() }))
    .sort((a, b) => a.r.top - b.r.top || a.r.left - b.r.left);
  const rows: DetailCell[][] = [];
  let bucket: { c: DetailCell; r: DOMRect }[] = [];
  let rowTop = -Infinity;
  const flush = () => {
    if (bucket.length) rows.push(bucket.sort((a, b) => a.r.left - b.r.left).map((x) => x.c));
  };
  for (const item of withRects) {
    if (item.r.top - rowTop > TOL) {
      flush();
      bucket = [item];
      rowTop = item.r.top;
    } else {
      bucket.push(item);
    }
  }
  flush();
  return rows;
}

/** A detail cell's horizontal center (viewport px) — the anchor for sticky-column
 *  vertical nav. */
export function cellCenterX(cell: DetailCell | undefined): number {
  if (!cell) return 0;
  const r = cell.el.getBoundingClientRect();
  return r.left + r.width / 2;
}

/** Index of the cell in `row` whose center sits nearest `x` — so j/k lands on the
 *  item spatially above/below where you were, not always the first one. */
export function nearestColTo(row: DetailCell[], x: number): number {
  let best = 0;
  let bestDist = Infinity;
  for (let i = 0; i < row.length; i++) {
    const dist = Math.abs(cellCenterX(row[i]) - x);
    if (dist < bestDist) {
      bestDist = dist;
      best = i;
    }
  }
  return best;
}
