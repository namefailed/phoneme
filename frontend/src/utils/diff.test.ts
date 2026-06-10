import { describe, it, expect } from "vitest";
import {
  tokenizeWords,
  tokenizeLines,
  diffTokens,
  diffText,
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
