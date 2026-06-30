import { describe, it, expect } from "vitest";
import { getByPath, setByPath } from "./form";

describe("form helpers", () => {
  it("getByPath resolves nested fields", () => {
    const config = {
      llm_post_process: {
        enabled: true,
        model: "llama3"
      }
    };
    
    expect(getByPath(config, "llm_post_process.enabled")).toBe(true);
    expect(getByPath(config, "llm_post_process.model")).toBe("llama3");
    expect(getByPath(config, "llm_post_process.missing")).toBeUndefined();
  });

  it("setByPath updates nested fields", () => {
    const config = {
      tray: {
        theme: "nord",
      },
      editor: {
        vim_mode: false
      }
    };
    
    setByPath(config, "tray.theme", "tokyo-night");
    setByPath(config, "editor.vim_mode", true);

    expect(config.tray.theme).toBe("tokyo-night");
    expect(config.editor.vim_mode).toBe(true);
  });

  it("setByPath throws when an intermediate object is missing (loud failure)", () => {
    // Documented contract: a typo'd field key (or unseeded config table) must
    // fail loudly in dev rather than silently dropping the user's edit. A
    // silent no-op regression would be caught here.
    expect(() => setByPath({}, "a.b", 1)).toThrow();
    // Deeper-missing link too: `a` exists but `a.b` doesn't.
    expect(() => setByPath({ a: {} }, "a.b.c", 1)).toThrow();
  });

  it("setByPath does NOT throw on a shallow (single-segment) write to a fresh object", () => {
    // A one-segment path has no intermediate to traverse, so it just assigns.
    const obj: Record<string, unknown> = {};
    setByPath(obj, "topLevel", 7);
    expect(obj.topLevel).toBe(7);
  });
});
