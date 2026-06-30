import { describe, it, expect } from "vitest";
import {
  formatDuration,
  statusToClass,
  statusLabel,
  escapeHtml,
  escapeAttr,
  highlightMatch,
  formatTime,
  wordCountSummary,
} from "./format";

describe("formatDuration", () => {
  it("formats zero as 0.0s", () => {
    expect(formatDuration(0)).toBe("0.0s");
  });

  it("formats sub-minute durations as decimal seconds", () => {
    expect(formatDuration(1500)).toBe("1.5s");
    expect(formatDuration(45000)).toBe("45.0s");
    expect(formatDuration(8470)).toBe("8.5s");
  });

  it("formats exactly 1 minute as 1m00s", () => {
    expect(formatDuration(60_000)).toBe("1m00s");
  });

  it("formats over-minute durations with zero-padded seconds", () => {
    expect(formatDuration(75_000)).toBe("1m15s");
    expect(formatDuration(65_000)).toBe("1m05s");
    expect(formatDuration(120_000)).toBe("2m00s");
  });

  it("formats hour-plus durations as Hh MMm", () => {
    expect(formatDuration(3_600_000)).toBe("1h00m"); // exactly 1h
    expect(formatDuration(3_665_000)).toBe("1h01m"); // 1h 1m 5s → seconds dropped at hour scale
    expect(formatDuration(9_000_000)).toBe("2h30m");
  });
});

describe("statusToClass", () => {
  it("maps 'done' to 'done'", () => {
    expect(statusToClass("done")).toBe("done");
  });

  it("maps terminal failure statuses to 'failed'", () => {
    expect(statusToClass("transcribe_failed")).toBe("failed");
    expect(statusToClass("hook_failed")).toBe("failed");
  });

  it("maps 'cancelled' to its own neutral class — never 'failed'", () => {
    expect(statusToClass("cancelled")).toBe("cancelled");
  });

  it("maps 'queued' to its own orange class — distinct from in-progress 'pending'", () => {
    expect(statusToClass("queued")).toBe("queued");
  });

  it("maps all in-progress statuses to 'pending'", () => {
    expect(statusToClass("recording")).toBe("pending");
    expect(statusToClass("transcribing")).toBe("pending");
    expect(statusToClass("cleaning_up")).toBe("pending");
    expect(statusToClass("summarizing")).toBe("pending");
    expect(statusToClass("tagging")).toBe("pending");
    expect(statusToClass("hook_running")).toBe("pending");
  });

  it("maps unknown status strings to 'pending'", () => {
    expect(statusToClass("some_future_status")).toBe("pending");
  });
});

describe("statusLabel", () => {
  it("returns a human-readable label for each known status", () => {
    expect(statusLabel("done")).toBe("Done");
    expect(statusLabel("transcribe_failed")).toBe("Transcription Failed");
    expect(statusLabel("hook_failed")).toBe("Hook Failed");
    expect(statusLabel("recording")).toBe("Recording");
    expect(statusLabel("cleaning_up")).toBe("Cleaning Up");
    expect(statusLabel("summarizing")).toBe("Summarizing");
    expect(statusLabel("tagging")).toBe("Tagging");
    expect(statusLabel("transcribing")).toBe("Transcribing");
    expect(statusLabel("hook_running")).toBe("Hook Running");
    expect(statusLabel("cancelled")).toBe("Cancelled");
  });

  it("returns the raw string for unknown statuses (passthrough)", () => {
    expect(statusLabel("custom_status")).toBe("custom_status");
  });
});

describe("escapeHtml", () => {
  it("escapes ampersands", () => {
    expect(escapeHtml("fish & chips")).toBe("fish &amp; chips");
  });

  it("escapes angle brackets", () => {
    expect(escapeHtml("<div>")).toBe("&lt;div&gt;");
  });

  it("escapes a full XSS payload", () => {
    expect(escapeHtml('<script>alert("xss")</script>')).toBe(
      '&lt;script&gt;alert("xss")&lt;/script&gt;'
    );
  });

  it("escapes multiple occurrences of the same character", () => {
    expect(escapeHtml("a < b & b > c")).toBe("a &lt; b &amp; b &gt; c");
  });

  it("leaves safe strings unchanged", () => {
    expect(escapeHtml("hello world")).toBe("hello world");
    expect(escapeHtml("")).toBe("");
  });
});

describe("highlightMatch", () => {
  it("returns plain-escaped text when term is empty", () => {
    expect(highlightMatch("hello <world>", "")).toBe("hello &lt;world&gt;");
  });

  it("returns plain-escaped text when the term is not found", () => {
    expect(highlightMatch("hello", "xyz")).toBe("hello");
  });

  it("wraps a single match in a mark element", () => {
    expect(highlightMatch("say hello", "hello")).toBe(
      'say <mark class="search-hit">hello</mark>'
    );
  });

  it("wraps all occurrences when there are multiple matches", () => {
    expect(highlightMatch("aa bb aa", "aa")).toBe(
      '<mark class="search-hit">aa</mark> bb <mark class="search-hit">aa</mark>'
    );
  });

  it("is case-insensitive and preserves original casing in the mark", () => {
    expect(highlightMatch("Say HELLO", "hello")).toBe(
      'Say <mark class="search-hit">HELLO</mark>'
    );
  });

  it("HTML-escapes the surrounding text while injecting marks", () => {
    expect(highlightMatch("<b>bold</b>", "bold")).toBe(
      '&lt;b&gt;<mark class="search-hit">bold</mark>&lt;/b&gt;'
    );
  });

  it("HTML-escapes text inside the matched portion", () => {
    expect(highlightMatch("say <b>hello</b>", "<b>")).toBe(
      'say <mark class="search-hit">&lt;b&gt;</mark>hello&lt;/b&gt;'
    );
  });

  it("handles regex special characters in the search term", () => {
    expect(highlightMatch("cost is $5.00", "$5.00")).toBe(
      'cost is <mark class="search-hit">$5.00</mark>'
    );
  });

  it("handles parentheses in the search term", () => {
    expect(highlightMatch("call foo()", "foo()")).toBe(
      'call <mark class="search-hit">foo()</mark>'
    );
  });
});

describe("wordCountSummary", () => {
  it("returns empty string for empty or whitespace-only text", () => {
    expect(wordCountSummary("")).toBe("");
    expect(wordCountSummary("   \n\t ")).toBe("");
  });

  it("uses singular labels for a single word / minute", () => {
    expect(wordCountSummary("hello")).toBe("1 word · ~1 min read");
  });

  it("counts whitespace-separated words and pluralizes", () => {
    expect(wordCountSummary("one two three")).toBe("3 words · ~1 min read");
  });

  it("collapses irregular whitespace when counting", () => {
    expect(wordCountSummary("  a   b\n\nc  ")).toBe("3 words · ~1 min read");
  });

  it("computes reading time at ~200 wpm (min 1 min)", () => {
    expect(wordCountSummary(Array(200).fill("w").join(" "))).toBe(
      "200 words · ~1 min read"
    );
    expect(wordCountSummary(Array(600).fill("w").join(" "))).toBe(
      "600 words · ~3 mins read"
    );
  });
});

describe("formatTime", () => {
  // ISO strings WITHOUT a trailing "Z" are parsed as LOCAL time, so
  // `toLocaleTimeString` (which renders in the local zone) produces the same
  // concrete output on any host timezone — no TZ pin needed, and the actual
  // hour:minute computation is pinned (not just "returns something").
  it("renders the local hour:minute in 24h mode", () => {
    expect(formatTime("2026-01-15T15:00:00", true)).toBe("15:00");
    expect(formatTime("2026-01-15T03:00:00", true)).toBe("03:00");
    expect(formatTime("2026-01-15T09:07:00", true)).toBe("09:07");
  });

  it("renders the local hour:minute with an AM/PM marker in 12h mode", () => {
    expect(formatTime("2026-01-15T15:00:00", false)).toMatch(/^03:00\s?PM$/i);
    expect(formatTime("2026-01-15T03:00:00", false)).toMatch(/^03:00\s?AM$/i);
  });

  it("produces different output for 12h vs 24h mode on the same timestamp", () => {
    expect(formatTime("2026-01-15T15:00:00", false)).toMatch(/03:00\s?PM/i);
    expect(formatTime("2026-01-15T15:00:00", true)).toBe("15:00");
  });

  it("24h output does not contain AM or PM markers", () => {
    const result = formatTime("2026-01-15T15:00:00", true);
    expect(result).not.toMatch(/\b(AM|PM)\b/i);
  });
});

describe("escapeAttr", () => {
  it("escapes the double-quote to &quot; (the attribute-breakout character)", () => {
    expect(escapeAttr('a "b" c')).toBe("a &quot;b&quot; c");
  });

  it("still escapes & < > like escapeHtml, plus the quote", () => {
    expect(escapeAttr('a "b" <c> & d')).toBe("a &quot;b&quot; &lt;c&gt; &amp; d");
  });

  it("neutralizes an attribute-breakout XSS payload", () => {
    expect(escapeAttr('"><script>')).toBe("&quot;&gt;&lt;script&gt;");
  });

  it("leaves safe strings unchanged", () => {
    expect(escapeAttr("hello world")).toBe("hello world");
    expect(escapeAttr("")).toBe("");
  });
});
