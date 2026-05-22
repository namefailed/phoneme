import { invoke } from "@tauri-apps/api/core";
import type { StepCallbacks } from "./Welcome";

export class ConfigureMode {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    body: HTMLElement,
    footer: HTMLElement,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private config: any,
    cbs: StepCallbacks,
  ) {
    const mode = this.config.llm.mode;
    if (mode === "external") {
      this.renderExternal(body, footer, cbs);
    } else if (mode === "bundled_model") {
      this.renderBundledModel(body, footer, cbs);
    } else {
      cbs.onNext();
    }
  }

  private renderExternal(body: HTMLElement, footer: HTMLElement, cbs: StepCallbacks) {
    body.innerHTML = `
      <h2 class="wizard-title">Point at your llama-server</h2>
      <p class="wizard-subtitle">Enter the URL of your running llama-server with an OpenAI-compatible API.</p>
      <div class="wizard-field">
        <label>Endpoint URL</label>
        <input type="text" id="url" value="${this.config.llm.external_url}" />
      </div>
      <button class="wizard-btn" id="test">Test connection</button>
      <div class="test-result" id="result" style="display:none"></div>
    `;
    this.renderFooter(footer, cbs);
    body.querySelector<HTMLInputElement>("#url")!.addEventListener("input", (e) => {
      this.config.llm.external_url = (e.target as HTMLInputElement).value;
    });
    body.querySelector("#test")?.addEventListener("click", async () => {
      const r = await invoke<{ ok: boolean; message: string }>("wizard_test_llm", {
        url: this.config.llm.external_url,
      });
      const el = body.querySelector<HTMLElement>("#result")!;
      el.style.display = "block";
      el.className = `test-result ${r.ok ? "ok" : "err"}`;
      el.textContent = r.message;
    });
  }

  private renderBundledModel(
    body: HTMLElement,
    footer: HTMLElement,
    cbs: StepCallbacks,
  ) {
    body.innerHTML = `
      <h2 class="wizard-title">Pick your model file</h2>
      <p class="wizard-subtitle">A GGUF model file (e.g., Gemma-4-E4B Q5_K_M).</p>
      <div class="wizard-field">
        <label>Model path</label>
        <input type="text" id="path" value="${this.config.llm.model_path}" />
        <button class="wizard-btn small" id="browse">Browse…</button>
      </div>
    `;
    this.renderFooter(footer, cbs);
    body.querySelector("#browse")?.addEventListener("click", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({
        multiple: false,
        filters: [{ name: "GGUF model", extensions: ["gguf"] }],
      });
      if (typeof path === "string") {
        this.config.llm.model_path = path;
        body.querySelector<HTMLInputElement>("#path")!.value = path;
      }
    });
    body.querySelector<HTMLInputElement>("#path")!.addEventListener("input", (e) => {
      this.config.llm.model_path = (e.target as HTMLInputElement).value;
    });
  }

  private renderFooter(footer: HTMLElement, cbs: StepCallbacks) {
    footer.innerHTML = `
      <button class="wizard-btn" id="back">← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn" id="skip">Skip setup</button>
      <button class="wizard-btn primary" id="next">Continue →</button>
    `;
    footer.querySelector("#back")?.addEventListener("click", () => cbs.onBack());
    footer.querySelector("#skip")?.addEventListener("click", () => cbs.onSkip());
    footer.querySelector("#next")?.addEventListener("click", () => cbs.onNext());
  }
}
