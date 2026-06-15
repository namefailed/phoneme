import { invoke } from "@tauri-apps/api/core";
import { curatedSttModels } from "../../services/sttProviders";
import { mountConnectionField } from "./connectionField";
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

  /** True when the FINAL transcription model is a heavy local model (medium /
   *  large). Sharing it for the preview ("same as final") is what makes the live
   *  text lag on a modest machine — so we nudge toward a dedicated tiny model.
   *  A cloud final model has no local CPU cost to share, so it's never "heavy"
   *  here. */
  private mainModelIsHeavy(): boolean {
    const w = this.config.whisper;
    if (!w) return false;
    if (w.provider && w.provider !== "local") return false;
    return /medium|large/i.test(String(w.model_path ?? ""));
  }

  /** The small downloaded model the local branch starts on: prefer a preview-
   *  sized one (Tiny/Base/Small), else any downloaded model, else blank (the
   *  daemon waits until one is picked — downloads happen in the Whisper
   *  section). */
  private firstLocalModel(): string {
    return (
      PREVIEW_MODELS.map((m) => this.downloadedPath(m.filename)).find(Boolean)
      ?? this.downloaded[0]
      ?? ""
    );
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
    // Feel/perf knobs (defaults mirror the daemon's serde defaults).
    const adaptive = this.config.recording?.preview_adaptive !== false;
    const waveform = this.config.recording?.preview_waveform !== false;
    const revealWps = this.config.recording?.preview_reveal_words_per_sec ?? 12;
    const idleMs = this.config.recording?.preview_idle_ms ?? 2500;

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Live Preview <span class="beta-pill" title="Live preview works but isn't smooth yet — a dedicated overhaul phase is on the roadmap. Off by default.">BETA</span></h3>
        <p style="font-size: 0.8571rem; color:var(--fg-muted); margin:0 0 4px;">
          Shows transcription as you speak. The preview runs on its <b>own fast model</b>, on a
          separate server from your final (high-quality) transcription — so a snappy live overlay
          never slows the real transcript down. (Dictation borrows this same fast model by default.)
        </p>

        <div class="settings-field">
          <label>Enable live preview</label>
          <div><input type="checkbox" class="toggle-switch" id="prev-enabled" ${enabled ? "checked" : ""} /></div>
        </div>

        <div class="settings-field">
          <label>System-wide overlay
            <br><span style="font-size: 0.7857rem; color:var(--fg-muted); font-weight:normal;">
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
          <label>Meetings (two tracks)
            <br><span style="font-size: 0.7857rem; color:var(--fg-muted); font-weight:normal;">
              How the overlay captions a meeting's mic + system audio.
            </span>
          </label>
          <div>
            <select id="prev-meeting-mode">
              <option value="toggle" ${(this.config.recording?.meeting_preview ?? "toggle") !== "both" ? "selected" : ""}>One track at a time — 🎤/🔊 toggle in the overlay (lighter)</option>
              <option value="both" ${this.config.recording?.meeting_preview === "both" ? "selected" : ""}>Both tracks at once — stacked captions (~double the preview work)</option>
            </select>
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

        <h4 style="margin:14px 0 6px; font-size: 0.9286rem; color:var(--fg-muted);">Feel &amp; performance</h4>

        <div class="settings-field">
          <label>Auto-throttle on slow machines
            <br><span style="font-size: 0.7857rem; color:var(--fg-muted); font-weight:normal;">
              When a preview update takes too long, the daemon automatically slows the cadence so
              recording never lags. Leave on unless you want a fixed update rate.
            </span>
          </label>
          <div><input type="checkbox" class="toggle-switch" id="prev-adaptive" ${adaptive ? "checked" : ""} ${enabled ? "" : "disabled"} /></div>
        </div>

        <div class="settings-field">
          <label>Reveal speed
            <br><span style="font-size: 0.7857rem; color:var(--fg-muted); font-weight:normal;">
              How fast words stream into the overlay, word by word. <b>Higher = a smoother crawl</b>
              (12 is a good default). <b>0 = each update appears instantly</b>, no smoothing — not a
              slower crawl. Applies to the recording overlay; dictation types straight at your cursor.
            </span>
          </label>
          <div>
            <input type="number" id="prev-reveal-wps" min="0" max="60" step="1" value="${revealWps}" style="width:90px;" ${enabled ? "" : "disabled"} />
            <span style="color:var(--fg-muted); font-size: 0.8571rem; margin-left:6px;">words / sec</span>
          </div>
        </div>

        <div class="settings-field">
          <label>Overlay waveform
            <br><span style="font-size: 0.7857rem; color:var(--fg-muted); font-weight:normal;">
              Show the “it hears me” bars in the desktop overlay so you can see audio is being captured,
              even between words.
            </span>
          </label>
          <div><input type="checkbox" class="toggle-switch" id="prev-waveform" ${waveform ? "checked" : ""} ${overlay ? "" : "disabled"} /></div>
        </div>

        <div class="settings-field">
          <label>“Listening” after
            <br><span style="font-size: 0.7857rem; color:var(--fg-muted); font-weight:normal;">
              When no new words arrive for this long, the overlay label calms from <b>LIVE</b> to <b>LISTENING</b>.
            </span>
          </label>
          <div>
            <input type="number" id="prev-idle-ms" min="500" max="20000" step="250" value="${idleMs}" style="width:110px;" ${overlay ? "" : "disabled"} />
            <span style="color:var(--fg-muted); font-size: 0.8571rem; margin-left:6px;">ms</span>
          </div>
        </div>
      </div>
    `;

    this.container.querySelector<HTMLInputElement>("#prev-enabled")?.addEventListener("change", (e) => {
      const on = (e.target as HTMLInputElement).checked;
      this.config.recording.streaming_preview = on;
      // The overlay needs preview text to show anything, so it only makes sense
      // alongside the preview. Turning the preview off also clears the overlay
      // flag and re-renders to disable its controls.
      if (!on && this.config.interface) this.config.interface.preview_overlay = false;
      // One-time nudge: enabling preview while it shares a heavy final model is
      // the classic "live preview lags my recording" trap. Steer toward a
      // dedicated tiny model once, then never nag again.
      if (on && this.source() === "same" && this.mainModelIsHeavy()) {
        try {
          if (!localStorage.getItem("phoneme.previewHeavyNudgeShown")) {
            showToast(
              "Live preview will share your heavy final model — for a smooth overlay, give it a dedicated Tiny model below.",
              "info",
            );
            localStorage.setItem("phoneme.previewHeavyNudgeShown", "1");
          }
        } catch {
          /* localStorage may be unavailable — the inline nudge below still shows */
        }
      }
      this.render();
    });

    this.container.querySelector<HTMLInputElement>("#prev-overlay")?.addEventListener("change", (e) => {
      if (this.config.interface) {
        this.config.interface.preview_overlay = (e.target as HTMLInputElement).checked;
      }
      this.render();
    });

    // "Preview" shows the overlay with sample text and keeps it up until the
    // user closes it with ✕ — all the time they need to drag and resize it.
    this.container.querySelector<HTMLButtonElement>("#prev-overlay-test")?.addEventListener("click", async () => {
      try {
        await invoke("set_overlay", { action: "preview" });
        showToast("Overlay shown with sample text — drag/resize it, then close it with ✕.", "info");
      } catch (e) {
        showToast(`Could not show overlay: ${errText(e)}`, "error");
      }
    });
    this.container.querySelector<HTMLSelectElement>("#prev-meeting-mode")?.addEventListener("change", (e) => {
      this.config.recording.meeting_preview = (e.target as HTMLSelectElement).value;
    });
    this.container.querySelector<HTMLSelectElement>("#prev-source")?.addEventListener("change", (e) => {
      const v = (e.target as HTMLSelectElement).value as PreviewSource;
      if (v === "same") this.setSame();
      else if (v === "local") this.setLocal(this.firstLocalModel());
      else this.setApi("groq");
      this.render();
    });

    // Feel/perf knobs. Toggles/number fields write straight through to
    // config.recording (persisted by the global Settings Save) and deliberately
    // do NOT re-render — re-rendering a focused number input would lose the caret.
    this.container.querySelector<HTMLInputElement>("#prev-adaptive")?.addEventListener("change", (e) => {
      this.config.recording.preview_adaptive = (e.target as HTMLInputElement).checked;
    });
    this.container.querySelector<HTMLInputElement>("#prev-waveform")?.addEventListener("change", (e) => {
      this.config.recording.preview_waveform = (e.target as HTMLInputElement).checked;
    });
    this.container.querySelector<HTMLInputElement>("#prev-reveal-wps")?.addEventListener("change", (e) => {
      const n = Number((e.target as HTMLInputElement).value);
      this.config.recording.preview_reveal_words_per_sec = Number.isFinite(n) ? Math.max(0, Math.min(60, n)) : 12;
    });
    this.container.querySelector<HTMLInputElement>("#prev-idle-ms")?.addEventListener("change", (e) => {
      const n = Number((e.target as HTMLInputElement).value);
      this.config.recording.preview_idle_ms = Number.isFinite(n) ? Math.max(500, Math.min(20000, Math.round(n))) : 2500;
    });

    this.renderDetail(src);
  }

  /** Shared model field for the api branch — re-mounted when the provider
   *  changes so the suggestions follow it. */
  private mountApiModel() {
    const modelHost = this.container.querySelector<HTMLElement>("#prev-api-model-host");
    if (!modelHost) return;
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

  private renderDetail(src: PreviewSource) {
    const host = this.container.querySelector<HTMLElement>("#prev-detail");
    if (!host) return;

    if (src === "same") {
      const heavy = this.mainModelIsHeavy();
      host.innerHTML = `
        <div class="settings-field">
          <label></label>
          <div style="font-size: 0.8571rem; color:var(--fg-muted); line-height:1.5;">
            Preview reuses your final model on the same server. Simplest, but on a heavy model the
            live text can lag — pick a dedicated local model or a cloud API for a snappy overlay.
          </div>
        </div>
        ${heavy ? `
        <div class="settings-field">
          <label></label>
          <div style="display:flex; flex-direction:column; gap:8px; padding:10px 12px; border:1px solid color-mix(in srgb, var(--accent, #89b4fa) 35%, transparent); background:color-mix(in srgb, var(--accent, #89b4fa) 10%, transparent); border-radius:8px; font-size: 0.8571rem; color:var(--fg-default); line-height:1.5;">
            <span>⚡ Your final model looks heavy for live preview. Give the preview its own Tiny model so recording stays smooth.</span>
            <div><button class="inline-button" id="prev-use-tiny">Use a dedicated Tiny model</button></div>
          </div>
        </div>` : ""}`;
      if (heavy) {
        host.querySelector<HTMLButtonElement>("#prev-use-tiny")?.addEventListener("click", () => {
          // Prefer an already-downloaded Tiny; else the lightest model on hand
          // (Whisper section handles downloads). Switching source re-renders into
          // the local branch where they can confirm/change the pick.
          const tiny = this.downloadedPath("ggml-tiny.en.bin") ?? this.firstLocalModel();
          this.setLocal(tiny);
          this.render();
        });
      }
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
        ${current || !this.downloaded.length ? "" : `<div class="settings-field"><label></label><div style="font-size: 0.8571rem; color:var(--err);">Pick a model above.</div></div>`}`;

      host.querySelector<HTMLSelectElement>("#prev-local-model")?.addEventListener("change", (e) => {
        const path = (e.target as HTMLSelectElement).value;
        if (path) {
          this.setLocal(path);
          this.render();
        }
      });
      return;
    }

    // Cloud API — the shared connection block (provider/key/Test/endpoint
    // override) + the shared model field. The block writes through setApi(),
    // which keeps the create semantics: a full WhisperConfig copy in external
    // mode, preserving any key/model/url already typed. Picking the local
    // provider in the block is the same as choosing "Dedicated local model"
    // in the source select — the section re-renders into that branch.
    host.innerHTML = `
      <div class="settings-field conn-field">
        <label>Provider</label>
        <div id="prev-api-conn"></div>
      </div>
      <div class="settings-field">
        <label>Model <span style="color:var(--fg-faded); font-weight:normal;">(optional)</span></label>
        <div id="prev-api-model-host"></div>
      </div>`;

    const connHost = host.querySelector<HTMLElement>("#prev-api-conn");
    if (connHost) {
      mountConnectionField(connHost, {
        catalog: "stt",
        getKind: () => this.config.preview_whisper?.provider ?? "",
        setKind: (k: string) => {
          if (k === "local") this.setLocal(this.firstLocalModel());
          else this.setApi(k);
        },
        getApiUrl: () => this.config.preview_whisper?.api_url ?? "",
        setApiUrl: (u: string) => { if (this.config.preview_whisper) this.config.preview_whisper.api_url = u; },
        getApiKey: () => this.config.preview_whisper?.api_key ?? "",
        setApiKey: (key: string) => { if (this.config.preview_whisper) this.config.preview_whisper.api_key = key; },
        onProviderChanged: () => {
          // A switch to the local provider changes the whole branch layout;
          // a cloud→cloud switch only needs the model suggestions to follow.
          if (this.source() !== "api") this.render();
          else this.mountApiModel();
        },
        resolveTestUrl: () => String(this.config.preview_whisper?.api_url ?? "").trim(),
      });
    }
    this.mountApiModel();
  }
}
