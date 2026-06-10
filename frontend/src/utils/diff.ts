/**
 * Tiny token-level diff used by the "Compare transcript versions" view.
 *
 * It computes a longest-common-subsequence (LCS) alignment between two token
 * lists and emits a flat list of {equal|insert|delete} ops. This is the textbook
 * diff algorithm (the same family `git`/`diff` use) — kept deliberately small and
 * dependency-free rather than pulling in a diff library for one read-only screen.
 *
 * Two tokenizers are provided:
 *   • word-level — splits on whitespace boundaries, KEEPING the trailing
 *     whitespace attached to each token, so concatenating the tokens reproduces
 *     the original text exactly. Good for prose where edits are a few words.
 *   • line-level — splits on newlines (keeping the trailing "\n"). Better when
 *     whole paragraphs were rewritten, where a word diff would look like noise.
 *
 * Complexity is O(n·m) in time and memory (full DP table). Transcripts are at
 * most a few thousand tokens, so this is fine; the caller caps the granularity
 * (word vs line) to keep it modest.
 */

export type DiffOpType = "equal" | "insert" | "delete";

export interface DiffOp {
  type: DiffOpType;
  /** The reconstructed text for this op (tokens joined back together). */
  value: string;
}

/**
 * Split text into word tokens, each carrying its trailing whitespace so the
 * tokens concatenate back to the original string. A leading run of whitespace
 * becomes its own token. Empty input yields an empty list.
 */
export function tokenizeWords(text: string): string[] {
  if (!text) return [];
  // Each match is a chunk of non-whitespace followed by any trailing whitespace,
  // OR a leading run of whitespace on its own. `join("")` is then loss-less.
  return text.match(/\s+|\S+\s*/g) ?? [];
}

/** Split text into line tokens, each keeping its trailing newline. */
export function tokenizeLines(text: string): string[] {
  if (!text) return [];
  // Keep the "\n" with the line so tokens concatenate back to the original.
  return text.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

/**
 * Diff two already-tokenized sequences via LCS and return a flat op list with
 * runs of the same type already coalesced. Equality is by exact token string.
 */
export function diffTokens(a: string[], b: string[]): DiffOp[] {
  const n = a.length;
  const m = b.length;

  // LCS length DP table: lcs[i][j] = LCS length of a[i:] and b[j:].
  // (n+1)·(m+1) so the last row/column are the zero base cases.
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }

  // Walk the table from the top-left, emitting one raw op per token. Prefer
  // deletions before insertions on ties so the output is deterministic.
  const raw: DiffOp[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      raw.push({ type: "equal", value: a[i] });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      raw.push({ type: "delete", value: a[i] });
      i++;
    } else {
      raw.push({ type: "insert", value: b[j] });
      j++;
    }
  }
  while (i < n) raw.push({ type: "delete", value: a[i++] });
  while (j < m) raw.push({ type: "insert", value: b[j++] });

  return coalesce(raw);
}

/** Merge adjacent ops of the same type into a single op (one DOM node each). */
function coalesce(ops: DiffOp[]): DiffOp[] {
  const out: DiffOp[] = [];
  for (const op of ops) {
    const last = out[out.length - 1];
    if (last && last.type === op.type) last.value += op.value;
    else out.push({ ...op });
  }
  return out;
}

export type DiffMode = "word" | "line";

/**
 * Convenience entry point: tokenize both sides at the requested granularity and
 * diff them. `a` is the "before"/left side, `b` is the "after"/right side, so
 * `delete` ops are text only in `a` and `insert` ops are text only in `b`.
 */
export function diffText(a: string, b: string, mode: DiffMode = "word"): DiffOp[] {
  const tok = mode === "line" ? tokenizeLines : tokenizeWords;
  return diffTokens(tok(a), tok(b));
}
