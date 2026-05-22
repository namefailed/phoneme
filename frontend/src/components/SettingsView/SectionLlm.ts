import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

export class SectionLlm {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>LLM</h3>
        <div class="settings-field">
          <label>Mode</label>
          <div>
            ${renderField(
              {
                key: "llm.mode",
                label: "Mode",
                kind: "select",
                options: [
                  { value: "external", label: "External (BYO server)" },
                  { value: "bundled_model", label: "Bundled server + my model" },
                  {
                    value: "bundled_download",
                    label: "Bundled server + downloaded model",
                  },
                ],
              },
              this.config.llm.mode,
            )}
          </div>
        </div>
        <div class="settings-field">
          <label>External URL</label>
          <div>
            ${renderField(
              { key: "llm.external_url", label: "", kind: "text" },
              this.config.llm.external_url,
            )}
            <button class="inline-button" id="test-llm">Test</button>
            <div class="test-result" id="llm-result" style="display:none"></div>
          </div>
        </div>
        <div class="settings-field">
          <label>Model file (.gguf)</label>
          <div>
            ${renderField(
              { key: "llm.model_path", label: "", kind: "text" },
              this.config.llm.model_path,
            )}
            <button class="inline-button" id="pick-model">Browse…</button>
          </div>
        </div>
        <div class="settings-field">
          <label>Timeout (seconds)</label>
          <div>${renderField(
            { key: "llm.timeout_secs", label: "", kind: "number" },
            this.config.llm.timeout_secs,
          )}</div>
        </div>
        <div class="settings-field">
          <label>System prompt</label>
          <div>${renderField(
            { key: "llm.system_prompt", label: "", kind: "textarea" },
            this.config.llm.system_prompt,
          )}</div>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    container.querySelector("#test-llm")?.addEventListener("click", async () => {
      const result = await invoke<{ ok: boolean; message: string }>("wizard_test_llm", {
        url: this.config.llm.external_url,
      });
      const el = container.querySelector<HTMLElement>("#llm-result")!;
      el.style.display = "block";
      el.className = `test-result ${result.ok ? "ok" : "err"}`;
      el.textContent = result.message;
    });

    container.querySelector("#pick-model")?.addEventListener("click", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({
        multiple: false,
        filters: [{ name: "GGUF model", extensions: ["gguf"] }],
      });
      if (typeof path === "string") {
        const input = container.querySelector<HTMLInputElement>(
          `[data-key="llm.model_path"]`,
        )!;
        input.value = path;
        this.config.llm.model_path = path;
      }
    });
  }
}
