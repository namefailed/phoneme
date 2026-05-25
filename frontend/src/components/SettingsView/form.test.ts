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
});
