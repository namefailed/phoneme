import { invoke } from "@tauri-apps/api/core";
import { PREVIEW_STT_PROVIDERS, curatedSttModels } from "../../services/sttProviders";
import { mountModelField } from "./modelField";
import { curatedTranscriptionModels } from "../../data/curatedModels";
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

/** Friendly label for a downloaded whisper model filename. */
function prettyModel(path: string): string {
  const name = path.replace(/\\/g, "/").split("/").pop() ?? path;
  const map: Record<string, string> = {
    "ggml-tiny.en.bin": "Tiny (English)",
    "ggml-base.en.bin": "Base (English)",
    "ggml-small.en.bin": "Small (English)",
    "ggml-medium.en.bin": "Medium (English)",
    "ggml-large-v3.bin": "Large v3",
    "ggml-large-v3-turbo.bin": "Large v3 Turbo",
    "ggml-large-v3-turbo-q5_0.bin": "Large v3 Turbo (q5)",
  };
  return map[name] ?? name;
}

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
    // Render synchronously so the section appears in order with the other
    // (synchronous) sections instead of popping in after the async model-list
    // fetch. The downloaded-model list then fills in via init().
    this.render();
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
    const overlay = !!this.config.interface?.preview_overlay;

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Live Preview</h3>
        <p style="font-size:12px; color:var(--fg-muted); margin:0 0 4px;">
          Shows transcription as you speak. Give it its own fast model or API so it
          never slows down your final transcription.
        </p>

        <div class="settings-field">
          <label>Enable live preview</label>
          <div><input type="checkbox" class="toggle-switch" id="prev-enabled" ${enabled ? "checked" : ""} /></div>
        </div>

        <div class="settings-field">
          <label>System-wide overlay
            <br><span style="font-size:11px; color:var(--fg-muted); font-weight:normal;">
              Float the live text in an always-on-top window over the whole desktop
              (draggable; remembers where you put it). Auto-shows when recording starts.
            </span>
          </label>
          <div style="display:flex; align-items:center; gap:10px;">
            <input type="checkbox" class="toggle-switch" id="prev-overlay" ${overlay ? "checked" : ""} ${enabled ? "" : "disabled"} />
            <button class="inline-button" id="prev-overlay-test" ${overlay ? "" : "disabled"}>Preview</button>
          </div>
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
      const on = (e.target as HTMLInputElement).checked;
      this.config.recording.streaming_preview = on;
      // The overlay needs preview text to show anything, so it only makes sense
      // alongside the preview. Turning the preview off also clears the overlay
      // flag and re-renders to disable its controls.
      if (!on && this.config.interface) this.config.interface.preview_overlay = false;
      this.render();
    });

    this.container.querySelector<HTMLInputElement>("#prev-overlay")?.addEventListener("change", (e) => {
      if (this.config.interface) {
        this.config.interface.preview_overlay = (e.target as HTMLInputElement).checked;
      }
      this.render();
    });

    // "Preview" briefly shows the overlay so the user can see and position it
    // without starting a real recording. Hides again after a few seconds.
    this.container.querySelector<HTMLButtonElement>("#prev-overlay-test")?.addEventListener("click", async () => {
      try {
        await invoke("set_overlay", { action: "show" });
        showToast("Overlay shown — drag it where you like; it hides shortly.", "info");
        setTimeout(() => void invoke("set_overlay", { action: "hide" }).catch(() => {}), 4000);
      } catch (e) {
        showToast(`Could not show overlay: ${errText(e)}`, "error");
      }
    });
    this.container.querySelector<HTMLSelectElement>("#prev-source")?.addEventListener("change", (e) => {
      const v = (e.target as HTMLSelectElement).value as PreviewSource;
      if (v === "same") this.setSame();
      else if (v === "local") {
        // Prefer a small downloaded model (Tiny/Base/Small) for a snappy preview;
        // else fall back to any downloaded model; else leave blank (the daemon
        // waits until one is picked — downloads happen in the Whisper section).
        const first = PREVIEW_MODELS.map((m) => this.downloadedPath(m.filename)).find(Boolean)
          ?? this.downloaded[0]
          ?? "";
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
      host.innerHTML = `
        <div class="settings-field">
          <label></label>
          <div style="font-size:12px; color:var(--fg-muted); line-height:1.5;">
            Preview reuses your final model on the same server. Simplest, but on a heavy model the
            live text can lag — pick a dedicated local model or a cloud API for a snappy overlay.
          </div>
        </div>`;
      return;
    }

    if (src === "local") {
      const current = this.config.preview_whisper?.model_path ?? "";
      const currentNorm = current.replace(/\\/g, "/");
      // Dropdown of every downloaded model. Downloading new models is handled in
      // the Whisper section above — this just picks which one drives the preview.
      const options = this.downloaded.length
        ? this.downloaded
            .map((p) => {
              const sel = currentNorm && currentNorm.endsWith(p.replace(/\\/g, "/").split("/").pop() ?? "") ? "selected" : "";
              return `<option value="${p.replace(/"/g, "&quot;")}" ${sel}>${prettyModel(p)}</option>`;
            })
            .join("")
        : `<option value="">No models downloaded yet</option>`;

      host.innerHTML = `
        <div class="settings-field">
          <label>Preview model</label>
          <div><select id="prev-local-model" style="width:100%; max-width:400px;">${options}</select></div>
          <span class="settings-help-text" style="grid-column:2;">
            Runs on a second, thread-limited whisper-server so it never lags your machine. Smaller
            models (Tiny / Base) give the snappiest overlay.${this.downloaded.length ? "" : " Download a model in the <b>Whisper</b> section above first."}
          </span>
        </div>
        ${current || !this.downloaded.length ? "" : `<div class="settings-field"><label></label><div style="font-size:12px; color:var(--err);">Pick a model above.</div></div>`}`;

      host.querySelector<HTMLSelectElement>("#prev-local-model")?.addEventListener("change", (e) => {
        const path = (e.target as HTMLSelectElement).value;
        if (path) {
          this.setLocal(path);
          this.render();
        }
      });
      return;
    }

    // Cloud API
    const pv = this.config.preview_whisper ?? {};
    host.innerHTML = `
      <div class="settings-field">
        <label>API provider</label>
        <div><select id="prev-api-provider">
          ${PREVIEW_STT_PROVIDERS.map(
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
        <div id="prev-api-model-host"></div>
      </div>
      <div class="settings-field">
        <label>API URL <span style="color:var(--fg-faded); font-weight:normal;">(optional)</span></label>
        <div><input type="text" id="prev-api-url" value="${pv.api_url ?? ""}" placeholder="provider default" style="width:100%;" /></div>
      </div>`;

    host.querySelector<HTMLSelectElement>("#prev-api-provider")?.addEventListener("change", (e) => {
      this.setApi((e.target as HTMLSelectElement).value);
      this.render();
    });
    host.querySelector<HTMLInputElement>("#prev-api-key")?.addEventListener("input", (e) => {
      if (this.config.preview_whisper) this.config.preview_whisper.api_key = (e.target as HTMLInputElement).value;
    });
    const modelHost = host.querySelector<HTMLElement>("#prev-api-model-host");
    if (modelHost) {
      mountModelField(modelHost, {
        mode: "curated",
        getProvider: () => this.config.preview_whisper?.provider ?? "",
        getApiUrl: () => this.config.preview_whisper?.api_url ?? "",
        getApiKey: () => this.config.preview_whisper?.api_key ?? "",
        getModel: () => this.config.preview_whisper?.model ?? "",
        setModel: (m) => { if (this.config.preview_whisper) this.config.preview_whisper.model = m; },
        curated: () => curatedSttModels(this.config.preview_whisper?.provider ?? ""),
        curatedRich: () => curatedTranscriptionModels(this.config.preview_whisper?.provider ?? ""),
      });
    }
    host.querySelector<HTMLInputElement>("#prev-api-url")?.addEventListener("input", (e) => {
      if (this.config.preview_whisper) this.config.preview_whisper.api_url = (e.target as HTMLInputElement).value;
    });
  }
}
