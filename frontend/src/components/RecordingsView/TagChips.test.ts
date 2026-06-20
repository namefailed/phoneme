import { describe, it, expect } from "vitest";
import { getContrastColor, safeTagColor } from "./TagChips";

describe("getContrastColor", () => {
  it("returns dark text color (#11111b) for light background colors", () => {
    expect(getContrastColor("#ffffff")).toBe("#11111b"); // pure white
    expect(getContrastColor("#ffff00")).toBe("#11111b"); // bright yellow
    expect(getContrastColor("#cdd6f4")).toBe("#11111b"); // light lavender
    expect(getContrastColor("#a6e3a1")).toBe("#11111b"); // light green
    expect(getContrastColor("#89b4fa")).toBe("#11111b"); // light blue
  });

  it("returns light text color (#ffffff) for dark background colors", () => {
    expect(getContrastColor("#000000")).toBe("#ffffff"); // pure black
    expect(getContrastColor("#11111b")).toBe("#ffffff"); // very dark crust
    expect(getContrastColor("#1e1e2e")).toBe("#ffffff"); // dark background
    expect(getContrastColor("#313244")).toBe("#ffffff"); // surface
    expect(getContrastColor("#ba1a1a")).toBe("#ffffff"); // dark red/rose
    expect(getContrastColor("#f38ba8")).toBe("#11111b"); // bright pinkish red has enough luminance
  });

  it("returns empty string for invalid or missing inputs", () => {
    expect(getContrastColor("")).toBe("");
    expect(getContrastColor("invalid")).toBe("");
    expect(getContrastColor("#gghhjj")).toBe(""); // non-hex characters
    expect(getContrastColor("123456")).toBe(""); // missing hash symbol
    expect(getContrastColor("#12")).toBe(""); // too short
  });
});

describe("safeTagColor", () => {
  it("passes valid hex colors through unchanged", () => {
    expect(safeTagColor("#fff")).toBe("#fff"); // #rgb
    expect(safeTagColor("#cba6f7")).toBe("#cba6f7"); // #rrggbb
    expect(safeTagColor("#cba6f7ff")).toBe("#cba6f7ff"); // #rrggbbaa
    expect(safeTagColor("#ABC")).toBe("#ABC"); // uppercase ok
  });

  it("falls back to the accent var for missing or non-hex input", () => {
    expect(safeTagColor(null)).toBe("var(--accent)");
    expect(safeTagColor(undefined)).toBe("var(--accent)");
    expect(safeTagColor("")).toBe("var(--accent)");
    expect(safeTagColor("red")).toBe("var(--accent)"); // named, not hex
    expect(safeTagColor("123456")).toBe("var(--accent)"); // missing hash
    expect(safeTagColor("#12")).toBe("var(--accent)"); // too short
    expect(safeTagColor("#1234567890")).toBe("var(--accent)"); // too long
  });

  it("rejects CSS-injection attempts that would break out of the style value", () => {
    // The whole point: a malicious tag color must not splice extra declarations.
    expect(safeTagColor("red; background:url(x)")).toBe("var(--accent)");
    expect(safeTagColor("#fff; position:fixed")).toBe("var(--accent)");
    expect(safeTagColor("#gghhjj")).toBe("var(--accent)"); // non-hex chars
  });

  it("honors a caller-supplied fallback", () => {
    expect(safeTagColor("bad", "#000")).toBe("#000");
    expect(safeTagColor("#abc", "#000")).toBe("#abc");
  });
});
