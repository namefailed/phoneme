import { describe, it, expect } from "vitest";
import { getContrastColor } from "./TagChips";

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
