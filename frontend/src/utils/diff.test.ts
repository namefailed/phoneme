import { describe, it, expect } from "vitest";
import {
  tokenizeWords,
  tokenizeLines,
  diffTokens,
  diffText,
  diffTextDetailed,
  MAX_LCS_CELLS,
  type DiffOp,
} from "./diff";

/** Reassemble the original "before" (a) text from a diff: equal + delete ops. */
function rebuildA(ops: DiffOp[]): string {
  return ops.filter((o) => o.type !== "insert").map((o) => o.value).join("");
}
/** Reassemble the "after" (b) text from a diff: equal + insert ops. */
function rebuildB(ops: DiffOp[]): string {
  return ops.filter((o) => o.type !== "delete").map((o) => o.value).join("");
}

describe("tokenizeWords", () => {
  it("returns an empty list for empty input", () => {
    expect(tokenizeWords("")).toEqual([]);
  });

  it("keeps trailing whitespace attached so tokens rejoin losslessly", () => {
    const text = "the quick  brown\nfox";
    expect(tokenizeWords(text).join("")).toBe(text);
  });

  it("captures a leading whitespace run as its own token", () => {
    expect(tokenizeWords("  hi")).toEqual(["  ", "hi"]);
  });
});

describe("tokenizeLines", () => {
  it("keeps the newline with each line and rejoins losslessly", () => {
    const text = "alpha\nbeta\ngamma";
    const toks = tokenizeLines(text);
    expect(toks).toEqual(["alpha\n", "beta\n", "gamma"]);
    expect(toks.join("")).toBe(text);
  });
});

describe("diffTokens", () => {
  it("marks two identical inputs entirely equal", () => {
    const ops = diffTokens(["a", "b", "c"], ["a", "b", "c"]);
    expect(ops).toEqual([{ type: "equal", value: "abc" }]);
  });

  it("detects a pure insertion", () => {
    const ops = diffTokens(["a", "c"], ["a", "b", "c"]);
    expect(ops).toEqual([
      { type: "equal", value: "a" },
      { type: "insert", value: "b" },
      { type: "equal", value: "c" },
    ]);
  });

  it("detects a pure deletion", () => {
    const ops = diffTokens(["a", "b", "c"], ["a", "c"]);
    expect(ops).toEqual([
      { type: "equal", value: "a" },
      { type: "delete", value: "b" },
      { type: "equal", value: "c" },
    ]);
  });

  it("represents a replacement as delete-then-insert", () => {
    const ops = diffTokens(["a", "b", "c"], ["a", "x", "c"]);
    expect(ops).toEqual([
      { type: "equal", value: "a" },
      { type: "delete", value: "b" },
      { type: "insert", value: "x" },
      { type: "equal", value: "c" },
    ]);
  });

  it("coalesces adjacent ops of the same type", () => {
    const ops = diffTokens(["a", "b"], ["x", "y"]);
    expect(ops).toEqual([
      { type: "delete", value: "ab" },
      { type: "insert", value: "xy" },
    ]);
  });

  it("treats an all-empty diff as no ops", () => {
    expect(diffTokens([], [])).toEqual([]);
  });

  it("emits only inserts when the left side is empty", () => {
    expect(diffTokens([], ["a", "b"])).toEqual([{ type: "insert", value: "ab" }]);
  });
});

describe("diffText", () => {
  it("word mode produces ops that rebuild both sides exactly", () => {
    const a = "the quick brown fox";
    const b = "the slow brown fox jumps";
    const ops = diffText(a, b, "word");
    expect(rebuildA(ops)).toBe(a);
    expect(rebuildB(ops)).toBe(b);
  });

  it("word mode isolates a single changed word", () => {
    const ops = diffText("hello world", "hello there", "word");
    // "hello " stays equal; "world" → "there".
    expect(ops.some((o) => o.type === "equal" && o.value.includes("hello"))).toBe(true);
    expect(ops.some((o) => o.type === "delete" && o.value.includes("world"))).toBe(true);
    expect(ops.some((o) => o.type === "insert" && o.value.includes("there"))).toBe(true);
  });

  it("line mode rebuilds multi-line text on both sides", () => {
    const a = "one\ntwo\nthree";
    const b = "one\nTWO\nthree\nfour";
    const ops = diffText(a, b, "line");
    expect(rebuildA(ops)).toBe(a);
    expect(rebuildB(ops)).toBe(b);
  });
});

describe("diffTextDetailed size guard", () => {
  it("runs the requested granularity exactly while under the cap", () => {
    const out = diffTextDetailed("hello world", "hello there", "word");
    expect(out.fallback).toBeNull();
    expect(out.ops.some((o) => o.type === "delete" && o.value.includes("world"))).toBe(true);
  });

  it("trims a long shared prefix/suffix so only the differing middle is charged", () => {
    // 12 shared words around a 1-word change: the middle is 1×1, so even a
    // 4-cell budget runs the exact word diff with no fallback.
    const pre = "alpha beta gamma delta epsilon zeta ";
    const suf = " eta theta iota kappa lambda mu";
    const a = `${pre}OLD${suf}`;
    const b = `${pre}NEW${suf}`;
    const out = diffTextDetailed(a, b, "word", 4);
    expect(out.fallback).toBeNull();
    expect(rebuildA(out.ops)).toBe(a);
    expect(rebuildB(out.ops)).toBe(b);
    expect(out.ops.some((o) => o.type === "delete" && o.value.includes("OLD"))).toBe(true);
    expect(out.ops.some((o) => o.type === "insert" && o.value.includes("NEW"))).toBe(true);
  });

  it("downgrades a too-large word diff to a line diff and says so", () => {
    // One differing 3-word line (3×3 = 9 cells > the 4-cell budget for words)
    // plus a shared line; as LINES the middle is 1×1, which fits.
    const a = "aaa bbb ccc\nsame line";
    const b = "xxx yyy zzz\nsame line";
    const out = diffTextDetailed(a, b, "word", 4);
    expect(out.fallback).toBe("line");
    expect(rebuildA(out.ops)).toBe(a);
    expect(rebuildB(out.ops)).toBe(b);
  });

  it("falls back to one coarse block when even lines exceed the cap", () => {
    // Force the block path with a zero budget; the shared first/last lines
    // must still come through as equal context around the block.
    const a = "same head\nold one\nold two\nsame tail";
    const b = "same head\nnew one\nnew two\nnew three\nsame tail";
    const out = diffTextDetailed(a, b, "line", 0);
    expect(out.fallback).toBe("block");
    expect(rebuildA(out.ops)).toBe(a);
    expect(rebuildB(out.ops)).toBe(b);
    expect(out.ops[0]).toEqual({ type: "equal", value: "same head\n" });
    expect(out.ops[out.ops.length - 1]).toEqual({ type: "equal", value: "same tail" });
    // Exactly one delete + one insert between the shared edges.
    expect(out.ops.filter((o) => o.type === "delete")).toHaveLength(1);
    expect(out.ops.filter((o) => o.type === "insert")).toHaveLength(1);
  });

  it("identical inputs stay a pure equal diff even with a zero budget", () => {
    const text = "nothing changed here\nat all";
    const out = diffTextDetailed(text, text, "word", 0);
    expect(out.fallback).toBeNull();
    expect(out.ops).toEqual([{ type: "equal", value: text }]);
  });

  it("meeting-length transcripts degrade instead of freezing (default cap)", () => {
    // Two ~3000-word texts with no overlap: 9M word-pairs > MAX_LCS_CELLS, so
    // the word diff must refuse the table. As single lines the middle is 1×1 —
    // the run degrades to a (here block-shaped) line diff and returns fast.
    const a = Array.from({ length: 3000 }, (_, i) => `left${i}`).join(" ");
    const b = Array.from({ length: 3000 }, (_, i) => `right${i}`).join(" ");
    expect(3000 * 3000).toBeGreaterThan(MAX_LCS_CELLS);
    const out = diffTextDetailed(a, b, "word");
    expect(out.fallback).toBe("line");
    expect(rebuildA(out.ops)).toBe(a);
    expect(rebuildB(out.ops)).toBe(b);
  });
});
