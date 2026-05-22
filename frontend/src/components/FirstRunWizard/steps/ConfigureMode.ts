import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
    const mode = this.config.whisper.mode;
    if (mode === "external") {
      this.renderExternal(body, footer, cbs);
    } else if (mode === "bundled_model") {
      this.renderBundledModel(body, footer, cbs);
    } else if (mode === "bundled_download") {
      this.renderBundledDownload(body, footer, cbs);
    } else {
      cbs.onNext();
    }
  }

  private renderExternal(body: HTMLElement, footer: HTMLElement, cbs: StepCallbacks) {
    body.innerHTML = `
      <h2 class="wizard-title">Point at your whisper-server</h2>
      <p class="wizard-subtitle">Enter the URL of your running whisper-server with an OpenAI-compatible API.</p>
      <div class="wizard-field">
        <label>Endpoint URL</label>
        <input type="text" id="url" value="${this.config.whisper.external_url}" />
      </div>
      <button class="wizard-btn" id="test">Test connection</button>
      <div class="test-result" id="result" style="display:none"></div>
    `;
    this.renderFooter(footer, cbs);
    body.querySelector<HTMLInputElement>("#url")!.addEventListener("input", (e) => {
      this.config.whisper.external_url = (e.target as HTMLInputElement).value;
    });
    body.querySelector("#test")?.addEventListener("click", async () => {
      const r = await invoke<{ ok: boolean; message: string }>("wizard_test_whisper", {
        url: this.config.whisper.external_url,
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
        <input type="text" id="path" value="${this.config.whisper.model_path}" />
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
        this.config.whisper.model_path = path;
        body.querySelector<HTMLInputElement>("#path")!.value = path;
      }
    });
    body.querySelector<HTMLInputElement>("#path")!.addEventListener("input", (e) => {
      this.config.whisper.model_path = (e.target as HTMLInputElement).value;
    });
  }

  private renderBundledDownload(
    body: HTMLElement,
    footer: HTMLElement,
    cbs: StepCallbacks,
  ) {
    body.innerHTML = `
      <h2 class="wizard-title" id="download-title">Downloading model</h2>
      <p class="wizard-subtitle" id="download-subtitle">Fetching the default Whisper model (ggml-base.en.bin) for transcription...</p>
      <div class="download-progress-container" style="margin-top:2rem;">
        <progress id="progress" max="100" value="0" style="width:100%;"></progress>
        <div id="status" style="text-align:center; font-size:12px; margin-top:8px;">Starting download...</div>
      </div>
    `;
    // Footer back button only while downloading, or disabled next until done
    footer.innerHTML = `
      <button class="wizard-btn" id="back" disabled>← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn primary" id="next" disabled>Continue →</button>
    `;

    // Download URL - hardcoded or from a list
    const url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";
    const filename = "ggml-base.en.bin";

    let unlisten: (() => void) | undefined;
    listen<{ downloaded: number; total: number | null }>("download_progress", (e) => {
      const p = body.querySelector<HTMLProgressElement>("#progress")!;
      const s = body.querySelector<HTMLElement>("#status")!;
      if (e.payload.total) {
        p.max = e.payload.total;
        p.value = e.payload.downloaded;
        s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      } else {
        p.removeAttribute("value");
        s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB downloaded`;
      }
    }).then((f) => {
      unlisten = f;
    });

    invoke<string>("wizard_download_model", { url, filename })
      .then(async (path) => {
        if (unlisten) unlisten();
        this.config.whisper.mode = "bundled_model";
        this.config.whisper.model_path = path;
        
        // Start server download
        body.querySelector<HTMLElement>("#download-title")!.textContent = "Downloading server";
        body.querySelector<HTMLElement>("#download-subtitle")!.textContent = "Fetching the Whisper server engine (approx 15MB)...";
        body.querySelector<HTMLProgressElement>("#progress")!.value = 0;
        body.querySelector<HTMLElement>("#status")!.textContent = "Starting server download...";

        let serverUnlisten: (() => void) | undefined;
        serverUnlisten = await listen<{ downloaded: number; total: number | null }>("server_download_progress", (e) => {
          const p = body.querySelector<HTMLProgressElement>("#progress")!;
          const s = body.querySelector<HTMLElement>("#status")!;
          if (e.payload.total) {
            p.max = e.payload.total;
            p.value = e.payload.downloaded;
            s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
          } else {
            p.removeAttribute("value");
            s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB downloaded`;
          }
        });

        return invoke<string>("wizard_download_server").then(() => {
          if (serverUnlisten) serverUnlisten();
        }).catch((err) => {
          if (serverUnlisten) serverUnlisten();
          throw err;
        });
      })
      .then(() => {
        body.querySelector<HTMLElement>("#status")!.textContent = "All downloads complete!";
        footer.querySelector<HTMLButtonElement>("#next")!.disabled = false;
        footer.querySelector<HTMLButtonElement>("#back")!.disabled = false;
        footer.querySelector("#next")?.addEventListener("click", () => cbs.onNext());
      })
      .catch((err) => {
        if (unlisten) unlisten();
        body.querySelector<HTMLElement>("#status")!.textContent = `Error: ${err}`;
        body.querySelector<HTMLElement>("#status")!.style.color = "red";
        footer.querySelector<HTMLButtonElement>("#back")!.disabled = false;
        footer.querySelector("#back")?.addEventListener("click", () => cbs.onBack());
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
