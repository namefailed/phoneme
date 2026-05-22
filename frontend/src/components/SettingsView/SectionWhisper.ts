import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

export class SectionWhisper {
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
        <h3>Whisper</h3>
        <div class="settings-field">
          <label>Mode</label>
          <div>
            ${renderField(
              {
                key: "whisper.mode",
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
              this.config.whisper.mode,
            )}
          </div>
        </div>
        <div class="settings-field">
          <label>External URL</label>
          <div>
            ${renderField(
              { key: "whisper.external_url", label: "", kind: "text" },
              this.config.whisper.external_url,
            )}
            <button class="inline-button" id="test-whisper">Test</button>
            <div class="test-result" id="whisper-result" style="display:none"></div>
          </div>
        </div>
        <div class="settings-field">
          <label>Model file</label>
          <div>
            ${renderField(
              { key: "whisper.model_path", label: "", kind: "text" },
              this.config.whisper.model_path,
            )}
            <button class="inline-button" id="pick-model">Browse…</button>
            <button class="inline-button" id="download-model">Download Default</button>
            <div id="download-status" style="display:none; font-size: 11px; margin-top: 4px;"></div>
          </div>
        </div>
        <div class="settings-field">
          <label>Timeout (seconds)</label>
          <div>${renderField(
            { key: "whisper.timeout_secs", label: "", kind: "number" },
            this.config.whisper.timeout_secs,
          )}</div>
        </div>
        <div class="settings-field">
          <label>System prompt</label>
          <div>${renderField(
            { key: "whisper.system_prompt", label: "", kind: "textarea" },
            this.config.whisper.system_prompt,
          )}</div>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    container.querySelector("#test-whisper")?.addEventListener("click", async () => {
      const result = await invoke<{ ok: boolean; message: string }>("wizard_test_whisper", {
        url: this.config.whisper.external_url,
      });
      const el = container.querySelector<HTMLElement>("#whisper-result")!;
      el.style.display = "block";
      el.className = `test-result ${result.ok ? "ok" : "err"}`;
      el.textContent = result.message;
    });

    container.querySelector("#pick-model")?.addEventListener("click", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({
        multiple: false,
        filters: [{ name: "Whisper model", extensions: ["bin"] }],
      });
      if (typeof path === "string") {
        const input = container.querySelector<HTMLInputElement>(
          `[data-key="whisper.model_path"]`,
        )!;
        input.value = path;
        this.config.whisper.model_path = path;
      }
    });

    container.querySelector("#download-model")?.addEventListener("click", async () => {
      const statusEl = container.querySelector<HTMLElement>("#download-status")!;
      statusEl.style.display = "block";
      statusEl.style.color = "var(--fg-muted)";
      statusEl.textContent = "Downloading ggml-base.en.bin...";
      
      const { listen } = await import("@tauri-apps/api/event");
      let unlisten: (() => void) | undefined;
      
      listen<{ downloaded: number; total: number | null }>("download_progress", (e) => {
        if (e.payload.total) {
          statusEl.textContent = `Downloading: ${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
        } else {
          statusEl.textContent = `Downloaded: ${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB`;
        }
      }).then((f) => {
        unlisten = f;
      });

      try {
        const path = await invoke<string>("wizard_download_model", {
          url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
          filename: "ggml-base.en.bin"
        });
        
        if (unlisten) unlisten();
        const input = container.querySelector<HTMLInputElement>(`[data-key="whisper.model_path"]`)!;
        input.value = path;
        this.config.whisper.model_path = path;
        
        statusEl.style.color = "var(--ok)";
        statusEl.textContent = "Download complete!";
      } catch (err) {
        if (unlisten) unlisten();
        statusEl.style.color = "var(--err)";
        statusEl.textContent = `Error: ${err}`;
      }
    });
  }
}
