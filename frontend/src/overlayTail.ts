// Pure helpers for the live-preview overlay's tentative-tail rendering (P2).
//
// The daemon tags each `transcription_partial` with `committed_len`: the char
// length of the STABLE prefix of `text`. Everything from there to the end is the
// freshly-appended, least-settled tail the overlay dims (`.tentative`) so the
// user can see which trailing words may still settle as more audio arrives.
//
// These functions are DOM- and Tauri-free on purpose: overlay.ts imports them,
// and they're unit-tested directly (overlay.ts itself can't be imported in a unit
// test — it touches the DOM and the Tauri event API at module load).

/** Tokenize into words (whitespace-separated). Empty/whitespace → []. */
export function toWords(text: string): string[] {
  const t = text.trim();
  return t ? t.split(/\s+/) : [];
}

/**
 * Number of leading WORDS of `text` that are committed (stable), given the char
 * length of the committed prefix. A word counts as committed only if it ENDS at
 * or before `committedLen` (a word straddling the boundary is treated as
 * tentative, so a mid-word offset never half-dims a word).
 *
 * Back-compat / edge rules:
 *  - `committedLen` undefined/null (older daemon, no field) → all words committed
 *    (render solid, exactly as before this field existed).
 *  - `committedLen` ≥ `text.length` → all committed (nothing new this tick).
 *  - `committedLen` ≤ 0 → 0 (first emit: everything is fresh).
 */
export function committedWordCount(
  text: string,
  committedLen: number | null | undefined,
): number {
  const words = toWords(text);
  if (committedLen == null) return words.length; // no field → all solid (back-compat)
  if (committedLen >= text.length) return words.length; // whole caption committed
  if (committedLen <= 0) return 0; // everything fresh (first emit)
  // Walk the original text tracking each word's END char index; a word is
  // committed while its end index is ≤ committedLen.
  let count = 0;
  let i = 0;
  const n = text.length;
  while (i < n && count < words.length) {
    while (i < n && /\s/.test(text[i])) i++; // skip spaces before the word
    if (i >= n) break;
    while (i < n && !/\s/.test(text[i])) i++; // consume the word
    if (i <= committedLen) count++;
    else break;
  }
  return count;
}

/**
 * Split a caption into its solid (committed) and tentative (freshly-appended)
 * portions for rendering, given the daemon's `committed_len`. Returns trimmed
 * word-joined strings so callers can drop straight into spans.
 *
 * Back-compat: `committedLen` undefined/null → everything solid, tentative "".
 * `committedLen` ≥ length → everything solid. This is the unit-tested helper that
 * proves the overlay's split-render is correct independent of the DOM.
 */
export function splitTentative(
  text: string,
  committedLen: number | null | undefined,
): { solid: string; tentative: string } {
  const words = toWords(text);
  const cut = committedWordCount(text, committedLen);
  return {
    solid: words.slice(0, cut).join(" "),
    tentative: words.slice(cut).join(" "),
  };
}
