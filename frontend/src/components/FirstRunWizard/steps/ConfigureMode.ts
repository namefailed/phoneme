import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { StepCallbacks } from "./Welcome";

export class ConfigureMode {
  constructor(
    private body: HTMLElement,
    private footer: HTMLElement,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private config: any,
    private cbs: StepCallbacks,
  ) {
    void this.runPipeline();
  }

  private async runPipeline() {
    const mode = this.config._setup_mode || "none";
    
    if (mode === "none") {
      this.config.whisper.mode = "external";
      this.cbs.onNext();
      return;
    }

    this.body.innerHTML = `
      <h2 class="wizard-title" id="download-title">Downloading models</h2>
      <p class="wizard-subtitle" id="download-subtitle">Please wait while Phoneme downloads and configures the required local AI models...</p>
      <div class="download-progress-container" style="margin-top:2rem;">
        <progress id="progress" max="100" value="0" style="width:100%;"></progress>
        <div id="status" style="text-align:center; font-size:12px; margin-top:8px;">Starting...</div>
      </div>
    `;
    this.footer.innerHTML = `
      <button class="wizard-btn" id="back" disabled>← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn primary" id="next" disabled>Continue →</button>
    `;

    try {
      if (mode === "whisper" || mode === "both") {
        await this.doWhisper();
      }
      if (mode === "ollama" || mode === "both") {
        await this.doOllama();
      }

      this.body.querySelector<HTMLElement>("#download-title")!.textContent = "Setup complete!";
      this.body.querySelector<HTMLElement>("#download-subtitle")!.textContent = "All components are installed and configured.";
      this.body.querySelector<HTMLElement>("#status")!.textContent = "Done.";
      this.body.querySelector<HTMLProgressElement>("#progress")!.value = 100;
      
      this.footer.querySelector<HTMLButtonElement>("#next")!.disabled = false;
      this.footer.querySelector<HTMLButtonElement>("#back")!.disabled = false;
      this.footer.querySelector("#next")?.addEventListener("click", () => this.cbs.onNext());
      this.footer.querySelector("#back")?.addEventListener("click", () => this.cbs.onBack());
    } catch (err) {
      console.error(err);
      this.body.querySelector<HTMLElement>("#download-title")!.textContent = "Setup failed";
      this.body.querySelector<HTMLElement>("#status")!.textContent = String(err);
      this.body.querySelector<HTMLElement>("#status")!.style.color = "red";
      this.footer.querySelector<HTMLButtonElement>("#back")!.disabled = false;
      this.footer.querySelector("#back")?.addEventListener("click", () => this.cbs.onBack());
    }
  }

  private async doWhisper() {
    this.body.querySelector<HTMLElement>("#download-title")!.textContent = "Whisper Setup";
    this.body.querySelector<HTMLElement>("#download-subtitle")!.textContent = "Fetching the default Whisper model (ggml-base.en.bin)...";
    this.body.querySelector<HTMLProgressElement>("#progress")!.value = 0;
    this.body.querySelector<HTMLElement>("#status")!.textContent = "Starting download...";

    const url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";
    const filename = "ggml-base.en.bin";

    let unlisten: (() => void) | undefined;
    unlisten = await listen<{ downloaded: number; total: number | null }>("download_progress", (e) => {
      const p = this.body.querySelector<HTMLProgressElement>("#progress")!;
      const s = this.body.querySelector<HTMLElement>("#status")!;
      if (e.payload.total) {
        p.max = e.payload.total;
        p.value = e.payload.downloaded;
        s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      } else {
        p.removeAttribute("value");
        s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB downloaded`;
      }
    });

    const path = await invoke<string>("wizard_download_model", { url, filename });
    if (unlisten) unlisten();

    this.config.whisper.mode = "bundled_model";
    this.config.whisper.model_path = path;
    
    // Server download
    this.body.querySelector<HTMLElement>("#download-subtitle")!.textContent = "Fetching the Whisper server engine (approx 15MB)...";
    this.body.querySelector<HTMLProgressElement>("#progress")!.value = 0;
    this.body.querySelector<HTMLElement>("#status")!.textContent = "Starting server download...";

    let serverUnlisten: (() => void) | undefined;
    serverUnlisten = await listen<{ downloaded: number; total: number | null }>("server_download_progress", (e) => {
      const p = this.body.querySelector<HTMLProgressElement>("#progress")!;
      const s = this.body.querySelector<HTMLElement>("#status")!;
      if (e.payload.total) {
        p.max = e.payload.total;
        p.value = e.payload.downloaded;
        s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      } else {
        p.removeAttribute("value");
        s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB downloaded`;
      }
    });

    try {
      await invoke<string>("wizard_download_server");
    } finally {
      if (serverUnlisten) serverUnlisten();
    }
  }

  private async doOllama() {
    this.body.querySelector<HTMLElement>("#download-title")!.textContent = "Ollama Setup";
    this.body.querySelector<HTMLElement>("#download-subtitle")!.textContent = "Checking if Ollama is running...";
    this.body.querySelector<HTMLProgressElement>("#progress")!.removeAttribute("value");
    this.body.querySelector<HTMLElement>("#status")!.textContent = "Pinging API...";

    const isRunning = await invoke<boolean>("wizard_ping_ollama");

    if (!isRunning) {
      this.body.querySelector<HTMLElement>("#download-subtitle")!.textContent = "Downloading Ollama installer...";
      this.body.querySelector<HTMLProgressElement>("#progress")!.value = 0;
      
      let unlisten: (() => void) | undefined;
      unlisten = await listen<{ downloaded: number; total: number | null }>("download_progress", (e) => {
        const p = this.body.querySelector<HTMLProgressElement>("#progress")!;
        const s = this.body.querySelector<HTMLElement>("#status")!;
        if (e.payload.total) {
          p.max = e.payload.total;
          p.value = e.payload.downloaded;
          s.textContent = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
        }
      });

      const installerPath = await invoke<string>("wizard_download_file", {
        url: "https://ollama.com/download/OllamaSetup.exe",
        filename: "OllamaSetup.exe",
      });
      if (unlisten) unlisten();

      this.body.querySelector<HTMLElement>("#download-subtitle")!.textContent = "Running Ollama installer. Please complete the setup window!";
      this.body.querySelector<HTMLProgressElement>("#progress")!.removeAttribute("value");
      this.body.querySelector<HTMLElement>("#status")!.textContent = "Waiting for Ollama to start...";

      await invoke("wizard_run_installer", { path: installerPath });

      // Poll until ping succeeds
      while (true) {
        await new Promise(r => setTimeout(r, 2000));
        const ok = await invoke<boolean>("wizard_ping_ollama");
        if (ok) break;
      }
    }

    this.body.querySelector<HTMLElement>("#download-subtitle")!.textContent = "Pulling Llama 3.2 (3B)...";
    this.body.querySelector<HTMLProgressElement>("#progress")!.value = 0;
    this.body.querySelector<HTMLElement>("#status")!.textContent = "Starting pull...";

    let pullUnlisten: (() => void) | undefined;
    pullUnlisten = await listen<{ status: string; completed: number | null; total: number | null }>("ollama_pull_progress", (e) => {
      const p = this.body.querySelector<HTMLProgressElement>("#progress")!;
      const s = this.body.querySelector<HTMLElement>("#status")!;
      s.textContent = e.payload.status;
      if (e.payload.total && e.payload.completed) {
        p.max = e.payload.total;
        p.value = e.payload.completed;
      }
    });

    try {
      await invoke("wizard_pull_ollama_model", { model: "llama3.2:3b" });
    } finally {
      if (pullUnlisten) pullUnlisten();
    }

    if (!this.config.llm_post_process) {
      this.config.llm_post_process = {};
    }
    this.config.llm_post_process.enabled = true;
    this.config.llm_post_process.provider = "ollama";
    this.config.llm_post_process.model = "llama3.2:3b";
    this.config.llm_post_process.api_url = "http://127.0.0.1:11434/api/generate";
  }
}
