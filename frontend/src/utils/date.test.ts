import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { formatDay } from "./date";

describe("formatDay", () => {
  beforeEach(() => {
    // Mock the system time to a fixed date so tests are deterministic
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-05-25T12:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns 'Today' for a date matching today", () => {
    expect(formatDay("2026-05-25T08:00:00Z")).toBe("Today");
    expect(formatDay("2026-05-25T18:00:00Z")).toBe("Today");
  });

  it("returns 'Yesterday' for exactly 1 day ago", () => {
    expect(formatDay("2026-05-24T12:00:00Z")).toBe("Yesterday");
  });

  it("returns 'Last 7 Days' for dates within 7 days", () => {
    expect(formatDay("2026-05-20T12:00:00Z")).toBe("Last 7 Days");
    expect(formatDay("2026-05-18T23:59:59Z")).toBe("Last 7 Days"); // exactly 7 days
  });

  it("returns 'Last 30 Days' for dates within 30 days", () => {
    expect(formatDay("2026-05-10T12:00:00Z")).toBe("Last 30 Days");
    expect(formatDay("2026-04-26T00:00:00Z")).toBe("Last 30 Days");
  });

  it("returns 'Older' for dates past 30 days", () => {
    expect(formatDay("2026-04-20T12:00:00Z")).toBe("Older");
    expect(formatDay("2025-05-25T12:00:00Z")).toBe("Older");
  });
});
