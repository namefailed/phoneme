import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// CSS imports are side-effect-only; stub them out under jsdom.
vi.mock("./modal.css", () => ({}));
vi.mock("./model-picker.css", () => ({}));
// Toast would touch timers / DOM we don't care about here.
vi.mock("../utils/toast", () => ({ showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import * as tauriCore from "@tauri-apps/api/core";
import { openModelPicker } from "./ModelPicker";

beforeEach(() => {
  vi.mocked(tauriCore.invoke).mockReset();
  // `read_config` is the first call; an empty object is enough because the
  // picker fills in the nested `whisper` / `llm_post_process` objects itself.
  vi.mocked(tauriCore.invoke).mockResolvedValue({});
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

function queryEl<T extends HTMLElement>(selector: string): T | null | undefined {
  // ModelPicker uses createRenderRoot() { return this; } so it renders to light DOM
  return document.querySelector("ph-model-picker")?.querySelector<T>(selector);
}

/** Cancel an open picker so its promise resolves and the DOM is cleaned up. */
function cancel() {
  queryEl<HTMLButtonElement>("#mp-cancel")!.click();
}

/** Wait for the picker's Lit render plus the shared model fields it mounts.
 *  llm-mode fields kick an async initial refresh; one macrotask lets it settle
 *  (the mocked / masked-key paths used here never touch the network). */
async function settled() {
  const el = document.querySelector("ph-model-picker") as
    | (HTMLElement & { updateComplete: Promise<unknown> })
    | null;
  await el?.updateComplete;
  await new Promise((r) => setTimeout(r, 0));
}

describe("openModelPicker", () => {
  it("renders a centered modal (not anchored) when no anchor is given", async () => {
    const p = openModelPicker("transcription");
    await vi.waitFor(() =>
      expect(queryEl(".modal-overlay")).toBeTruthy(),
    );

    const overlay = queryEl(".modal-overlay")!;
    expect(overlay.classList.contains("mp-anchored")).toBe(false);
    // A centered modal does not get inline positioning.
    const dialog = queryEl<HTMLElement>(".mp-dialog")!;
    expect(dialog.style.top).toBe("");

    cancel();
    await expect(p).resolves.toBe(false);
    expect(queryEl(".modal-overlay")).toBeFalsy();
  });

  it("renders as an anchored dropdown when an anchor element is given", async () => {
    const anchor = document.createElement("button");
    document.body.appendChild(anchor);

    const p = openModelPicker("transcription", anchor);
    await vi.waitFor(() =>
      expect(queryEl(".modal-overlay")).toBeTruthy(),
    );

    const overlay = queryEl(".modal-overlay")!;
    expect(overlay.classList.contains("mp-anchored")).toBe(true);
    // Anchored mode positions the dialog beneath the trigger via inline styles.
    const dialog = queryEl<HTMLElement>(".mp-dialog")!;
    expect(dialog.style.top).not.toBe("");
    expect(dialog.style.left).not.toBe("");

    cancel();
    await expect(p).resolves.toBe(false);
  });

  it("opens on the requested tab", async () => {
    const p = openModelPicker("postprocessing");
    await vi.waitFor(() =>
      expect(queryEl(".mp-tab.active")).toBeTruthy(),
    );

    const active = queryEl(".mp-tab.active")!;
    expect(active.textContent).toContain("Post-processing");

    cancel();
    await p;
  });
});

describe("model slots", () => {
  it("mounts the shared connection block and model field into every slot host", async () => {
    const p = openModelPicker("transcription");
    await vi.waitFor(() =>
      expect(queryEl(".modal-overlay")).toBeTruthy(),
    );
    await settled();

    // One connection host + one model host per slot — whisper/STT, cleanup
    // LLM, summary, auto-tag, live preview — each filled by the shared blocks
    // (.cf-provider / .mf-select), not hand-rolled per-slot controls.
    for (const slot of ["stt", "llm", "sum", "at", "prev"]) {
      const conn = queryEl<HTMLElement>(`#mp-${slot}-conn-host`);
      expect(conn, `connection host div for "${slot}" slot`).toBeTruthy();
      expect(conn!.querySelector(".cf-provider"), `connection block in "${slot}" slot`).toBeTruthy();
      const host = queryEl<HTMLElement>(`#mp-${slot}-model-host`);
      expect(host, `host div for "${slot}" slot`).toBeTruthy();
      expect(host!.querySelector(".mf-select"), `shared field in "${slot}" slot`).toBeTruthy();
    }

    cancel();
    await p;
  });

  it("shows the saved cleanup model as selected and derives the matching named provider", async () => {
    vi.mocked(tauriCore.invoke).mockImplementation(async (cmd: string): Promise<any> => {
      if (cmd === "read_config") {
        return {
          llm_post_process: {
            provider: "groq",
            // The Groq entry's exact endpoint → the provider select derives it.
            api_url: "https://api.groq.com/openai/v1/chat/completions",
            // Masked key, as the daemon hands it to the WebView — the field
            // skips the live fetch, so this never touches the network.
            api_key: "__phoneme_secret_kept__",
            model: "llama-3.1-8b-instant",
          },
        };
      }
      if (cmd === "wizard_list_downloaded_models") return [];
      return {};
    });

    const p = openModelPicker("postprocessing");
    await vi.waitFor(() =>
      expect(queryEl(".modal-overlay")).toBeTruthy(),
    );
    await settled();

    const modelSelect = queryEl<HTMLSelectElement>("#mp-llm-model-host .mf-select")!;
    expect(modelSelect).toBeTruthy();
    expect(modelSelect.value).toBe("llama-3.1-8b-instant");

    const provider = queryEl<HTMLSelectElement>("#mp-llm-conn-host .cf-provider")!;
    expect(provider).toBeTruthy();
    expect(provider.value).toBe("groq");
    expect(provider.selectedOptions[0]?.textContent).toContain("Groq");
    // The saved key round-trips masked — never cleared behind the user's back.
    const key = queryEl<HTMLInputElement>("#mp-llm-conn-host .cf-key")!;
    expect(key.value).toBe("__phoneme_secret_kept__");

    cancel();
    await p;
  });

  it("saving an untouched default config writes the same keys as before", async () => {
    const p = openModelPicker("transcription");
    await vi.waitFor(() =>
      expect(queryEl("#mp-save")).toBeTruthy(),
    );
    queryEl<HTMLButtonElement>("#mp-save")!.click();
    await expect(p).resolves.toBe(true);

    const write = vi.mocked(tauriCore.invoke).mock.calls.find(([cmd]) => cmd === "write_config");
    expect(write).toBeTruthy();
    const cfg = (write![1] as { config: any }).config;
    expect(cfg.whisper).toEqual({ provider: "local", model: "", api_key: "", api_url: "" });
    expect(cfg.llm_post_process).toEqual({ provider: "none", model: "", api_key: "", api_url: "", enabled: false });
    // An empty config reads as diarization-on (provider unset ≠ "none") → "local".
    expect(cfg.diarization).toEqual({ provider: "local" });
    expect(cfg.summary).toEqual({ provider: "", model: "", api_key: "", api_url: "" });
    expect(cfg.auto_tag).toEqual({ provider: "", model: "", api_key: "", api_url: "" });
    expect(cfg.semantic_search).toEqual({ model_dir: "" });
    expect(cfg.preview_whisper).toBeNull();
  });
});
