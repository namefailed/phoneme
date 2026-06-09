import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../../utils/toast";
import { errText } from "../../utils/error";

/** Small, fast models suitable for the live preview (the final transcript keeps
 *  whatever the Transcription section is set to). */
const PREVIEW_MODELS = [
  { filename: "ggml-tiny.en.bin", name: "Tiny (English)", size: "75 MB", desc: "Fastest — best for a snappy overlay on any machine." },
  { filename: "ggml-base.en.bin", name: "Base (English)", size: "142 MB", desc: "A little more accurate live text, still light." },
  { filename: "ggml-small.en.bin", name: "Small (English)", size: "466 MB", desc: "Better live text; needs a bit more CPU." },
];
const HF_BASE = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/** Cloud providers that work well for a fast preview (final stays separate). */
const PREVIEW_API_PROVIDERS = [
  { value: "groq", label: "Groq (fast, recommended)" },
  { value: "openai", label: "OpenAI" },
  { value: "deepgram", label: "Deepgram" },
  { value: "custom", label: "Custom (OpenAI-compatible)" },
];

type PreviewSource = "same" | "local" | "api";

/**
 * Live-preview configuration. The preview can run on its own provider so it
 * never contends with the (heavy) final model:
 *   • Same as final — reuse the main provider/server (default).
 *   • Dedicated local model — a small model on its OWN bundled server.
 *   • Cloud API — a fast API (e.g. Groq).
 * Writes `config.recording.streaming_preview` + `config.preview_whisper`; the
 * global Settings Save persists via write_config. The daemon spins up / tears
 * down the second whisper-server based on this (see preview_needs_own_server).
 */
export class SectionPreview {
  private downloaded: string[] = [];

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(private container: HTMLElement, private config: any) {
    void this.init();
  }

  private async init() {
    try {
      this.downloaded = await invoke<string[]>("wizard_list_downloaded_models");
    } catch {
      this.downloaded = [];
    }
    this.render();
  }

  private source(): PreviewSource {
    const pv = this.config.preview_whisper;
    if (!pv) return "same";
    return pv.provider === "local" ? "local" : "api";
  }

  /** Full path of an already-downloaded model file, or null. */
  private downloadedPath(filename: string): string | null {
    return this.downloaded.find((p) => p.replace(/\\/g, "/").endsWith(filename)) ?? null;
  }

  private mainPort(): number {
    return (this.config.whisper?.bundled_server_port ?? 5809) as number;
  }

  /** Build a preview WhisperConfig from the main one + overrides (so every
   *  required field is present), for a local bundled model. */
  private setLocal(modelPath: string) {
    this.config.preview_whisper = {
      ...this.config.whisper,
      provider: "local",
      mode: "bundled_model",
      model_path: modelPath,
      // Distinct port from the final server so both run concurrently.
      bundled_server_port: this.mainPort() + 1,
      api_key: "",
    };
  }

  private setApi(provider: string) {
    const existing = this.config.preview_whisper ?? {};
    this.config.preview_whisper = {
      ...this.config.whisper,
      provider,
      mode: "external",
      model_path: "",
      api_key: existing.api_key ?? "",
      model: existing.model ?? "",
      api_url: existing.api_url ?? "",
    };
  }

  private setSame() {
    delete this.config.preview_whisper;
  }

  private render() {
    const src = this.source();
    const enabled = !!this.config.recording?.streaming_preview;

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Live Preview</h3>
        <p style="font-size:12px; color:var(--fg-muted); margin:0 0 4px;">
          Shows transcription as you speak. Give it its own fast model or API so it
          never slows down your final transcription.
        </p>

        <div class="settings-field">
          <label>Enable live preview</label>
          <div><input type="checkbox" id="prev-enabled" ${enabled ? "checked" : ""} /></div>
        </div>

        <div class="settings-field">
          <label>Preview source</label>
          <div>
            <select id="prev-source">
              <option value="same" ${src === "same" ? "selected" : ""}>Same as final model</option>
              <option value="local" ${src === "local" ? "selected" : ""}>Dedicated local model (recommended)</option>
              <option value="api" ${src === "api" ? "selected" : ""}>Cloud API</option>
            </select>
          </div>
        </div>

        <div id="prev-detail"></div>
      </div>
    `;

    this.container.querySelector<HTMLInputElement>("#prev-enabled")?.addEventListener("change", (e) => {
      this.config.recording.streaming_preview = (e.target as HTMLInputElement).checked;
    });
    this.container.querySelector<HTMLSelectElement>("#prev-source")?.addEventListener("change", (e) => {
      const v = (e.target as HTMLSelectElement).value as PreviewSource;
      if (v === "same") this.setSame();
      else if (v === "local") {
        // Default to the first already-downloaded preview model, else clear path
        // (the daemon waits until a model is selected/downloaded).
        const first = PREVIEW_MODELS.map((m) => this.downloadedPath(m.filename)).find(Boolean) ?? "";
        this.setLocal(first);
      } else this.setApi("groq");
      this.render();
    });

    this.renderDetail(src);
  }

  private renderDetail(src: PreviewSource) {
    const host = this.container.querySelector<HTMLElement>("#prev-detail");
    if (!host) return;

    if (src === "same") {
      host.innerHTML = `<p style="font-size:12px; color:var(--fg-muted); padding:8px 0;">
        Preview reuses your final model on the same server. Simplest, but on a heavy
        model the live text can lag. Pick a dedicated model or API for a snappy overlay.</p>`;
      return;
    }

    if (src === "local") {
      const current = this.config.preview_whisper?.model_path ?? "";
      const rows = PREVIEW_MODELS.map((m) => {
        const path = this.downloadedPath(m.filename);
        const selected = current && current.replace(/\\/g, "/").endsWith(m.filename);
        const action = path
          ? `<button class="inline-button" data-pick="${path}">${selected ? "Selected" : "Use"}</button>`
          : `<button class="inline-button" data-dl="${m.filename}">Download</button>`;
        return `<div class="settings-field" style="border:0; padding:6px 0;">
            <label style="font-weight:normal;">${m.name} <span style="color:var(--fg-faded);">${m.size}</span><br>
              <span style="font-size:11px; color:var(--fg-muted);">${m.desc}</span></label>
            <div>${action}</div>
          </div>`;
      }).join("");
      host.innerHTML = `
        <p style="font-size:12px; color:var(--fg-muted); padding:4px 0;">
          Runs on a second whisper-server (thread-limited so it can't lag your machine).</p>
        ${rows}
        ${current ? "" : `<p style="font-size:12px; color:var(--err);">Pick or download a model above.</p>`}`;

      host.querySelectorAll<HTMLButtonElement>("[data-pick]").forEach((b) =>
        b.addEventListener("click", () => {
          this.setLocal(b.dataset.pick!);
          this.render();
        }),
      );
      host.querySelectorAll<HTMLButtonElement>("[data-dl]").forEach((b) =>
        b.addEventListener("click", async () => {
          const filename = b.dataset.dl!;
          b.disabled = true;
          b.textContent = "Downloading…";
          try {
            const path = await invoke<string>("wizard_download_model", {
              url: `${HF_BASE}/${filename}`,
              filename,
            });
            this.downloaded.push(path);
            this.setLocal(path);
            showToast(`Downloaded ${filename}`, "success");
            this.render();
          } catch (e) {
            showToast(`Download failed: ${errText(e)}`, "error");
            b.disabled = false;
            b.textContent = "Download";
          }
        }),
      );
      return;
    }

    // Cloud API
    const pv = this.config.preview_whisper ?? {};
    host.innerHTML = `
      <div class="settings-field">
        <label>API provider</label>
        <div><select id="prev-api-provider">
          ${PREVIEW_API_PROVIDERS.map(
            (p) => `<option value="${p.value}" ${pv.provider === p.value ? "selected" : ""}>${p.label}</option>`,
          ).join("")}
        </select></div>
      </div>
      <div class="settings-field">
        <label>API key</label>
        <div><input type="password" id="prev-api-key" value="${pv.api_key ?? ""}" style="width:100%;" /></div>
      </div>
      <div class="settings-field">
        <label>Model <span style="color:var(--fg-faded); font-weight:normal;">(optional)</span></label>
        <div><input type="text" id="prev-api-model" value="${pv.model ?? ""}" placeholder="provider default" style="width:100%;" /></div>
      </div>
      <div class="settings-field">
        <label>API URL <span style="color:var(--fg-faded); font-weight:normal;">(optional)</span></label>
        <div><input type="text" id="prev-api-url" value="${pv.api_url ?? ""}" placeholder="provider default" style="width:100%;" /></div>
      </div>`;

    host.querySelector<HTMLSelectElement>("#prev-api-provider")?.addEventListener("change", (e) => {
      this.setApi((e.target as HTMLSelectElement).value);
    });
    host.querySelector<HTMLInputElement>("#prev-api-key")?.addEventListener("input", (e) => {
      if (this.config.preview_whisper) this.config.preview_whisper.api_key = (e.target as HTMLInputElement).value;
    });
    host.querySelector<HTMLInputElement>("#prev-api-model")?.addEventListener("input", (e) => {
      if (this.config.preview_whisper) this.config.preview_whisper.model = (e.target as HTMLInputElement).value;
    });
    host.querySelector<HTMLInputElement>("#prev-api-url")?.addEventListener("input", (e) => {
      if (this.config.preview_whisper) this.config.preview_whisper.api_url = (e.target as HTMLInputElement).value;
    });
  }
}
