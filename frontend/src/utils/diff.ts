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
 * Complexity is O(n·m) in time and memory (full DP table) — fine for prose-
 * sized inputs but lethal on hour-long meeting transcripts (tens of thousands
 * of words per side → billions of DP cells → a frozen webview). `diffText` /
 * `diffTextDetailed` therefore peel off the common prefix/suffix first (cheap,
 * and most edits are local) and cap the remaining table at [`MAX_LCS_CELLS`];
 * past the cap a word diff degrades to a line diff, and past that to a single
 * coarse "block" delete+insert of the differing middle. `diffTextDetailed`
 * reports which fallback (if any) was taken so the UI can say so.
 */

/** The three diff operations. "delete" = only in the left/before text,
 *  "insert" = only in the right/after text. */
export type DiffOpType = "equal" | "insert" | "delete";

/** One coalesced run of same-typed tokens in the diff output. */
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
 * Hard ceiling on the LCS DP table (left tokens × right tokens), applied by
 * `diffTextDetailed` to the post-trim middles. ~4M cells stays well under
 * 100ms and a few tens of MB; the unbounded table on long meeting transcripts
 * (e.g. 20k × 20k words = 400M cells) froze the webview outright.
 */
export const MAX_LCS_CELLS = 4_000_000;

/**
 * Diff two already-tokenized sequences via LCS and return a flat op list with
 * runs of the same type already coalesced. Equality is by exact token string.
 * NOTE: raw and uncapped — O(a.length · b.length) time AND memory. Text-sized
 * inputs should go through `diffText`/`diffTextDetailed`, which trim shared
 * edges and enforce [`MAX_LCS_CELLS`].
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

/** Diff granularity (see the tokenizer notes in the module comment). */
export type DiffMode = "word" | "line";

/** Length of the common prefix and suffix of two token lists (exact token
 *  equality); the suffix scan stops before it would overlap the prefix. */
function commonEdges(a: string[], b: string[]): { pre: number; suf: number } {
  const max = Math.min(a.length, b.length);
  let pre = 0;
  while (pre < max && a[pre] === b[pre]) pre++;
  let suf = 0;
  while (suf < max - pre && a[a.length - 1 - suf] === b[b.length - 1 - suf]) suf++;
  return { pre, suf };
}

/**
 * LCS diff with the shared edges peeled off first: the common prefix/suffix
 * become plain equal ops and only the differing middle pays for the O(n·m)
 * table. Returns null when even that middle would exceed `maxCells`.
 */
function diffTrimmed(a: string[], b: string[], maxCells: number): DiffOp[] | null {
  const { pre, suf } = commonEdges(a, b);
  const midA = a.slice(pre, a.length - suf);
  const midB = b.slice(pre, b.length - suf);
  if (midA.length * midB.length > maxCells) return null;
  const ops: DiffOp[] = [];
  if (pre) ops.push({ type: "equal", value: a.slice(0, pre).join("") });
  ops.push(...diffTokens(midA, midB));
  if (suf) ops.push({ type: "equal", value: a.slice(a.length - suf).join("") });
  return coalesce(ops);
}

/** Coarsest possible diff — shared edges kept, the whole differing middle as
 *  one delete + one insert. Always linear; the last-resort fallback. */
function blockDiff(a: string[], b: string[]): DiffOp[] {
  const { pre, suf } = commonEdges(a, b);
  const ops: DiffOp[] = [];
  if (pre) ops.push({ type: "equal", value: a.slice(0, pre).join("") });
  const delMid = a.slice(pre, a.length - suf).join("");
  const insMid = b.slice(pre, b.length - suf).join("");
  if (delMid) ops.push({ type: "delete", value: delMid });
  if (insMid) ops.push({ type: "insert", value: insMid });
  if (suf) ops.push({ type: "equal", value: a.slice(a.length - suf).join("") });
  return coalesce(ops);
}

/** A size-guarded diff result: the ops plus which fallback (if any) ran. */
export interface DiffOutcome {
  ops: DiffOp[];
  /**
   * Which size guard kicked in, if any: "line" = the requested word diff was
   * too large and a line diff was rendered instead; "block" = even the line
   * version blew the cap and the differing middle is one coarse delete+insert.
   * null = the requested granularity ran exactly.
   */
  fallback: "line" | "block" | null;
}

/**
 * Size-guarded diff: tokenize at the requested granularity, trim the shared
 * prefix/suffix, and run the LCS only when the remaining table fits in
 * `maxCells` (see [`MAX_LCS_CELLS`]). A too-large word diff retries at line
 * granularity (1–2 orders fewer tokens); a too-large line diff degrades to a
 * single block. Every path keeps the rebuild invariant: equal+delete ops
 * concatenate to `a`, equal+insert ops to `b`.
 */
export function diffTextDetailed(
  a: string,
  b: string,
  mode: DiffMode = "word",
  maxCells: number = MAX_LCS_CELLS,
): DiffOutcome {
  const tok = mode === "line" ? tokenizeLines : tokenizeWords;
  const exact = diffTrimmed(tok(a), tok(b), maxCells);
  if (exact) return { ops: exact, fallback: null };
  if (mode === "word") {
    const la = tokenizeLines(a);
    const lb = tokenizeLines(b);
    const byLine = diffTrimmed(la, lb, maxCells);
    if (byLine) return { ops: byLine, fallback: "line" };
    return { ops: blockDiff(la, lb), fallback: "block" };
  }
  return { ops: blockDiff(tok(a), tok(b)), fallback: "block" };
}

/**
 * Convenience entry point: tokenize both sides at the requested granularity and
 * diff them. `a` is the "before"/left side, `b` is the "after"/right side, so
 * `delete` ops are text only in `a` and `insert` ops are text only in `b`.
 * Size-guarded — see [`diffTextDetailed`] to learn whether a fallback ran.
 */
export function diffText(a: string, b: string, mode: DiffMode = "word"): DiffOp[] {
  return diffTextDetailed(a, b, mode).ops;
}
