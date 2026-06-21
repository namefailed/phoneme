/**
 * Lightweight client-side fuzzy matching for filtering small in-memory lists
 * (tag names, settings fields, etc.).
 *
 * Deliberately separate from the main transcript search, which runs as SQLite
 * FTS5 inside the database over recording text. These helpers run in the
 * browser over arrays already in memory, where a tiny subsequence matcher is
 * the right tool.
 */

/**
 * Score how well `query` fuzzy-matches `target`. Higher is better.
 * Returns `null` when there's no match at all.
 *
 * Ranking: a contiguous substring beats a scattered subsequence, an
 * earlier/word-boundary match beats a later one, and consecutive matched
 * characters are rewarded.
 */
export function fuzzyScore(query: string, target: string): number | null {
  const q = query.trim().toLowerCase();
  const t = target.toLowerCase();
  if (!q) return 0; // empty query matches everything

  // Substring match scores highest; earlier and word-boundary positions win.
  const idx = t.indexOf(q);
  if (idx !== -1) {
    const boundary = idx === 0 || /\s|[-_/]/.test(t[idx - 1]) ? 100 : 0;
    return 1000 + boundary - idx;
  }

  // Subsequence match: every query char must appear in order.
  let ti = 0;
  let score = 0;
  let prevMatchedAt = -2;
  for (let qi = 0; qi < q.length; qi++) {
    const ch = q[qi];
    let found = -1;
    while (ti < t.length) {
      if (t[ti] === ch) { found = ti; break; }
      ti++;
    }
    if (found === -1) return null; // char missing in order → no match
    score += found === prevMatchedAt + 1 ? 3 : 1; // reward consecutive hits
    prevMatchedAt = found;
    ti = found + 1;
  }
  return score;
}

/** Whether `query` fuzzy-matches `target` at all. */
export function fuzzyMatch(query: string, target: string): boolean {
  return fuzzyScore(query, target) !== null;
}

/**
 * Filter + rank `items` by how well `selector(item)` fuzzy-matches `query`.
 * An empty query returns the items unchanged (original order).
 */
export function fuzzyFilter<T>(query: string, items: T[], selector: (item: T) => string): T[] {
  if (!query.trim()) return items;
  return items
    .map((item) => ({ item, score: fuzzyScore(query, selector(item)) }))
    .filter((r): r is { item: T; score: number } => r.score !== null)
    .sort((a, b) => b.score - a.score)
    .map((r) => r.item);
}
