import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";
import { escapeAttr, escapeHtml } from "../../utils/format";
import { sttMeta, curatedSttModels } from "../../services/sttProviders";
import { mountConnectionField } from "./connectionField";
import { mountModelField } from "./modelField";
import { curatedTranscriptionModels } from "../../data/curatedModels";

const HELP =
  "font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;";

/** Whisper's prompt budget: the text decoder context is 448 tokens for every
 *  model size and the prompt is limited to `n_text_ctx/2 - 1 = 223`; OpenAI's
 *  API rounds this to 224. We cap the custom-vocabulary box at this many tokens
 *  (counted with Whisper's own GPT-2 / r50k BPE) rather than guessing from
 *  character count — dense jargon tokenizes at ~2.7 chars/token, far from the
 *  ~4 the old 900-char cap assumed. */
const VOCAB_MAX_TOKENS = 224;
/** Coarse character ceiling applied before tokenizing, purely so a pathological
 *  paste can't hand the encoder a megabyte of text. The token cap above is the
 *  real limit and always bites first for any realistic vocab. */
const VOCAB_CHAR_BACKSTOP = 8000;

type VocabTokenizer = { encode: (text: string) => number[]; decode: (tokens: number[]) => string };
let _vocabTokenizer: Promise<VocabTokenizer> | null = null;
/** Lazily load Whisper's BPE tokenizer (r50k_base == the GPT-2 byte-level BPE
 *  Whisper is built on) the first time the vocab box is shown, so its ~1 MB of
 *  rank data never lands in the main bundle. */
function loadVocabTokenizer(): Promise<VocabTokenizer> {
  if (!_vocabTokenizer) {
    _vocabTokenizer = import("gpt-tokenizer/encoding/r50k_base") as unknown as Promise<VocabTokenizer>;
  }
  return _vocabTokenizer;
}

/** The port fields a `DaemonStatus` reply carries for the bundled whisper
 *  servers. The `preferred` ports are the configured `bundled_server_port`
 *  values; the `effective` ports are what the supervisors ACTUALLY bound —
 *  they fall back to a free port when a foreign app holds the preferred one —
 *  and are `null` while that server isn't running. Mirrors the daemon's
 *  `DaemonStatus` reply (crates/phoneme-ipc/src/schema.rs). All fields are
 *  optional so a partial/old reply, or a probe against a down daemon, simply
 *  yields "no fallback known" rather than throwing. */
export interface WhisperPortStatus {
  whisper_preferred_port?: number | null;
  whisper_effective_port?: number | null;
  preview_whisper_preferred_port?: number | null;
  preview_whisper_effective_port?: number | null;
}

/** A configured port that the daemon actually bound elsewhere: the live
 *  `effective` port plus a short human note explaining the fallback. */
export interface EffectivePort {
  /** The port the server is really listening on right now. */
  effective: number;
  /** The configured port the user picked, which was busy. */
  preferred: number;
  /** Ready-to-show note, e.g. "(running on 51234 — preferred 5809 was busy)". */
  note: string;
}

/**
 * Decide which port to actually SHOW for a configured local-whisper port.
 *
 * Pure display logic — the editable config value never changes. Mirrors the
 * tray's `effective_local_whisper_url` (src-tauri/src/commands.rs): for either
 * supervised server (main or live-preview), when the configured port matches a
 * reported `preferred` port and the live `effective` port differs and is
 * known, the server fell back to a free port — return the effective port and a
 * note. Otherwise (no status, daemon down, ports equal, server not running, or
 * an unrelated port) returns `null`, so the caller keeps showing the
 * configured port unchanged.
 */
export function effectivePortFor(
  configuredPort: number,
  status: WhisperPortStatus | null | undefined,
): EffectivePort | null {
  if (!status) return null;
  const pairs: [number | null | undefined, number | null | undefined][] = [
    [status.whisper_preferred_port, status.whisper_effective_port],
    [status.preview_whisper_preferred_port, status.preview_whisper_effective_port],
  ];
  for (const [preferred, effective] of pairs) {
    if (
      typeof preferred === "number" &&
      typeof effective === "number" &&
      preferred === configuredPort &&
      effective !== preferred
    ) {
      return {
        effective,
        preferred,
        note: `(running on ${effective} — preferred ${preferred} was busy)`,
      };
    }
  }
  return null;
}

/** Rewrite a `http://127.0.0.1:<port>` URL to the port the daemon actually
 *  bound, when it fell back. Returns the original URL untouched for any other
 *  shape or when no fallback applies. The matching `note` (or `""`) rides
 *  alongside so callers can append it to a hint. */
export function effectiveLocalWhisperHint(
  url: string,
  status: WhisperPortStatus | null | undefined,
): { url: string; note: string } {
  const m = url.trim().match(/^http:\/\/127\.0\.0\.1:(\d+)\/?$/);
  if (!m) return { url, note: "" };
  const port = Number(m[1]);
  const eff = effectivePortFor(port, status);
  if (!eff) return { url, note: "" };
  return { url: `http://127.0.0.1:${eff.effective}`, note: eff.note };
}

const MODELS = [
  { id: "tiny", filename: "ggml-tiny.en.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin", name: "Tiny", size: "75 MB", desc: "Fastest, lowest accuracy. Good for quick dictation." },
  { id: "base", filename: "ggml-base.en.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin", name: "Base", size: "142 MB", desc: "Fast, decent accuracy. Good balance for older machines." },
  { id: "small", filename: "ggml-small.en.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin", name: "Small", size: "466 MB", desc: "Moderate speed, good accuracy. Standard choice." },
  { id: "medium", filename: "ggml-medium.en.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin", name: "Medium", size: "1.5 GB", desc: "Slower, great accuracy. Recommended for modern PCs." },
  // Turbo before full Large v3: it's smaller (1.6 GB vs 3.1 GB) and faster, so
  // it sits lower on the resource ladder despite the shared "Large v3" name.
  { id: "large-v3-turbo", filename: "ggml-large-v3-turbo.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin", name: "Large v3 Turbo", size: "1.6 GB", desc: "Fast and highly accurate. Great high-accuracy pick for most modern PCs." },
  { id: "large-v3", filename: "ggml-large-v3.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin", name: "Large v3", size: "3.1 GB", desc: "Slowest, best accuracy. High-end hardware only." }
];

/**
 * Settings → Transcription: the main speech-to-text engine (`config.whisper`).
 * Provider choice via the shared connection block (local whisper.cpp / the
 * cloud providers / custom endpoint — see services/sttProviders), the shared
 * model field with curated per-provider suggestions, and — for the local
 * engine — downloadable whisper model cards with size/accuracy notes, a
 * "recommended for your RAM" pick (`wizard_get_system_info`), download
 * progress, and the currently-downloaded check
 * (`wizard_list_downloaded_models`). Plain section class composing the
 * shared connectionField/modelField mounts over the form.ts binding.
 */
export class SectionWhisper {

  constructor(
    private container: HTMLElement,
    private config: any,
  ) {
    this.render(container);
    void this.fetchHardwareAndModels();
    void this.refreshEffectivePort();
  }

  /** Ask the daemon which port the bundled main server actually bound and, if
   *  it fell back from the configured one, surface a small note next to the
   *  local-server hint. Best-effort: a down daemon or a partial reply just
   *  leaves the note empty. Calls `daemon_status` directly (the typed services
   *  wrapper drops the port fields) — pure display, the config is untouched. */
  private async refreshEffectivePort() {
    const slot = this.container.querySelector<HTMLElement>("#whisper-effective-port");
    if (!slot) return;
    const w = this.config.whisper ?? {};
    // Only meaningful for a bundled local server (no fixed port for external/cloud).
    if (String(w.provider ?? "local") !== "local" || w.mode === "external") {
      slot.textContent = "";
      return;
    }
    const configuredPort = (w.bundled_server_port ?? 5809) as number;
    try {
      const status = await invoke<WhisperPortStatus>("daemon_status");
      const eff = effectivePortFor(configuredPort, status);
      slot.textContent = eff
        ? `The server is currently ${eff.note.replace(/^\(|\)$/g, "")}.`
        : "";
    } catch {
      slot.textContent = "";
    }
  }

  private async fetchHardwareAndModels() {
    try {
      const sysInfo = await invoke<{ ram_mb: number }>("wizard_get_system_info");
      const downloaded = await invoke<string[]>("wizard_list_downloaded_models");

      let recommendedId = "base";
      if (sysInfo.ram_mb >= 16000) recommendedId = "large-v3";
      else if (sysInfo.ram_mb >= 8000) recommendedId = "medium";
      else if (sysInfo.ram_mb >= 4000) recommendedId = "small";

      this.updateModelCards(downloaded, recommendedId);
    } catch (e) {
      console.error("Failed to fetch hardware/model info", e);
    }
  }

  private updateModelCards(downloadedPaths: string[], recommendedId: string) {
    MODELS.forEach((m) => {
      // It's downloaded if any path ends with the filename
      const downloadedPath = downloadedPaths.find(p => p.endsWith(m.filename));
      const isDownloaded = !!downloadedPath;
      const isSelected = this.config.whisper.model_path === downloadedPath;

      const card = this.container.querySelector(`#model-card-${m.id}`);
      if (!card) return;

      const badgeArea = card.querySelector(".model-badge");
      if (badgeArea) {
        if (m.id === recommendedId) {
          badgeArea.innerHTML = `<span style="background: rgba(166,227,161,0.2); color: var(--ok); padding: 2px 6px; border-radius: 4px; font-size: 0.6429rem; font-weight: bold;">⭐ RECOMMENDED</span>`;
        }
      }

      const btnArea = card.querySelector(".model-actions");
      if (btnArea) {
        if (isSelected) {
          btnArea.innerHTML = `<button disabled style="background: var(--accent); color: var(--bg-surface); border-color: var(--accent); border-radius: 6px;">✅ Selected</button>`;
        } else if (isDownloaded) {
          btnArea.innerHTML = `<button class="select-btn" data-id="${escapeAttr(m.id)}" data-path="${escapeAttr(downloadedPath ?? "")}" style="border-radius: 6px;">Select</button>`;
        } else {
          btnArea.innerHTML = `
            <button class="download-btn" data-id="${m.id}" data-url="${m.url}" data-filename="${m.filename}" style="border-radius: 6px;">
              Download
            </button>
            <div class="progress-text" style="display:none; font-size: 0.7143rem; color: var(--fg-muted); margin-top: 4px;"></div>
          `;
        }
      }
    });

    // Re-bind dynamically generated buttons
    this.container.querySelectorAll(".select-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        const path = (btn as HTMLElement).dataset.path!;
        this.config.whisper.model_path = path;
        // Trigger a fake change event on the hidden input to notify config store
        const input = this.container.querySelector<HTMLInputElement>(`[data-key="whisper.model_path"]`);
        if (input) {
          input.value = path;
          input.dispatchEvent(new Event("change", { bubbles: true }));
        }
        // Optimistic UI update
        this.updateModelCards(downloadedPaths, recommendedId);
      });
    });

    this.container.querySelectorAll(".download-btn").forEach((btn) => {
      btn.addEventListener("click", async (e) => {
        const target = e.currentTarget as HTMLButtonElement;
        const url = target.dataset.url!;
        const filename = target.dataset.filename!;
        const progressEl = target.parentElement?.querySelector(".progress-text") as HTMLElement;

        target.disabled = true;
        target.textContent = "Downloading...";
        if (progressEl) {
          progressEl.style.display = "block";
          progressEl.textContent = "0 MB";
        }

        const { listen } = await import("@tauri-apps/api/event");
        const unlisten = await listen<{ downloaded: number; total: number | null }>("download_progress", (ev) => {
          if (progressEl) {
            if (ev.payload.total) {
              progressEl.textContent = `${(ev.payload.downloaded / 1024 / 1024).toFixed(1)} / ${(ev.payload.total / 1024 / 1024).toFixed(1)} MB`;
            } else {
              progressEl.textContent = `${(ev.payload.downloaded / 1024 / 1024).toFixed(1)} MB`;
            }
          }
        });

        try {
          const newPath = await invoke<string>("wizard_download_model", { url, filename });
          downloadedPaths.push(newPath);
          // Auto-select after download
          this.config.whisper.model_path = newPath;
          const input = this.container.querySelector<HTMLInputElement>(`[data-key="whisper.model_path"]`);
          if (input) {
            input.value = newPath;
            input.dispatchEvent(new Event("change", { bubbles: true }));
          }
        } catch (err) {
          console.error(err);
          if (progressEl) progressEl.textContent = "Error downloading.";
        } finally {
          if (unlisten) unlisten();
          this.updateModelCards(downloadedPaths, recommendedId);
        }
      });
    });
  }

  /**
   * What the connection block's Test button probes. For the local provider
   * this mirrors the daemon's `server_base_url()` exactly: external mode →
   * the configured endpoint, bundled modes → the supervised server's port.
   * For the custom provider it's the OpenAI-compatible base URL.
   */
  private testUrl(): string {
    const w = this.config.whisper ?? {};
    if (String(w.provider ?? "local") !== "local") return String(w.api_url ?? "").trim();
    return w.mode === "external"
      ? String(w.external_url ?? "").replace(/\/+$/, "")
      : `http://127.0.0.1:${w.bundled_server_port ?? 5809}`;
  }

  private render(container: HTMLElement) {
    const modelCardsHtml = MODELS.map(m => `
      <div id="model-card-${m.id}" style="display: flex; justify-content: space-between; align-items: center; padding: 6px 10px; border: 1px solid var(--border-subtle); border-radius: 6px; margin-bottom: 4px; background: var(--bg-deep);">
        <div style="display: flex; flex-direction: column; gap: 2px;">
          <div style="display: flex; align-items: center; gap: 8px;">
            <strong style="color: var(--fg-default); font-size: 0.9286rem;">${m.name}</strong>
            <span style="color: var(--fg-faded); font-size: 0.7857rem;">${m.size}</span>
            <div class="model-badge"></div>
          </div>
          <span style="font-size: 0.7857rem; color: var(--fg-muted);">${m.desc}</span>
        </div>
        <div class="model-actions" style="display: flex; flex-direction: column; align-items: flex-end;">
           <span style="font-size: 0.7857rem; color: var(--fg-faded);">Loading...</span>
        </div>
      </div>
    `).join("");

    container.innerHTML = `
      <div class="settings-section">
        <h3>Whisper</h3>
        <!-- Shared connection block: provider select (grouped local/cloud/
             advanced), key row when the provider needs one, Test, and the
             cloud endpoint override under its Advanced fold. The model row
             stays ours, below, because local "models" are files on disk. -->
        <div class="settings-field conn-field">
          <label>Provider</label>
          <div id="whisper-connection"></div>
        </div>

        <div id="whisper-cloud" style="display:none">
          <div class="settings-field long-input">
            <label>Model</label>
            <div id="whisper-model-host"></div>
            <span style="${HELP}" id="cloud-model-help">
              Leave blank to use the provider default.
            </span>
          </div>
        </div>

        <div id="whisper-local">
          <div class="settings-field" style="align-items: start;">
            <label>Bundled Model</label>
            <!-- Hidden input to maintain form binding -->
            <div style="display:none;">
              ${renderField(
                { key: "whisper.model_path", label: "", kind: "text" },
                this.config.whisper.model_path,
              )}
            </div>
            <div style="display: flex; flex-direction: column; gap: 4px; align-items: stretch; max-width: 580px;">
              ${modelCardsHtml}
              <div style="margin-top: 8px;">
                 <button class="inline-button" id="pick-model" style="font-size: 0.7857rem;">Browse for custom .bin…</button>
              </div>
              <span style="${HELP}">
                Models run locally via <code>whisper.cpp</code>. Larger models have higher accuracy but use more RAM and run slower.
              </span>
            </div>
          </div>

          <details class="settings-advanced">
            <summary>
              <svg class="settings-advanced-chev" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true">
                <path d="M9 6l6 6-6 6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
              Advanced — local server connection
            </summary>
            <span style="display:block; font-size: 0.7857rem; color:var(--fg-faded); margin:4px 0 10px;">
              Normally the app starts and manages its own whisper server for the model picked
              above — nothing to configure. Fill the URL below only to use a server you run yourself.
            </span>
            <span id="whisper-effective-port" style="display:block; font-size: 0.7857rem; color:var(--accent); margin:0 0 10px;"></span>
            <div class="settings-field long-input">
              <label>External URL</label>
              <div>${renderField(
                { key: "whisper.external_url", label: "", kind: "text" },
                this.config.whisper.external_url,
              )}</div>
              <span style="${HELP}">
                The endpoint to send audio to when using <b>External</b> mode. Must be a Whisper-compatible API (e.g., <code>http://127.0.0.1:8080/inference</code>). The Test button above checks whichever endpoint is in effect.
              </span>
            </div>
          </details>
        </div>

        <div class="settings-field">
          <label>Timeout (seconds)</label>
          <div>${renderField(
            { key: "whisper.timeout_secs", label: "", kind: "number" },
            this.config.whisper.timeout_secs,
          )}</div>
          <span style="${HELP}">
            Maximum time (in seconds) to wait for the transcription to complete before giving up and labeling the recording as failed.
          </span>
        </div>
        <div class="settings-field">
          <label>Language</label>
          <div>${renderField(
            {
              key: "whisper.language",
              label: "",
              kind: "select",
              options: [
                { value: "",   label: "Auto-detect (recommended)" },
                { value: "en", label: "English" },
                { value: "es", label: "Spanish" },
                { value: "fr", label: "French" },
                { value: "de", label: "German" },
                { value: "it", label: "Italian" },
                { value: "pt", label: "Portuguese" },
                { value: "nl", label: "Dutch" },
                { value: "ru", label: "Russian" },
                { value: "ja", label: "Japanese" },
                { value: "zh", label: "Chinese" },
                { value: "ko", label: "Korean" },
                { value: "ar", label: "Arabic" },
                { value: "hi", label: "Hindi" },
                { value: "tr", label: "Turkish" },
                { value: "pl", label: "Polish" },
                { value: "uk", label: "Ukrainian" },
                { value: "sv", label: "Swedish" },
                { value: "da", label: "Danish" },
                { value: "fi", label: "Finnish" },
                { value: "no", label: "Norwegian" },
              ],
            },
            this.config.whisper.language ?? "",
          )}</div>
          <span style="${HELP}">
            Hint the language of your speech to improve accuracy. Leave on <b>Auto-detect</b> if you record in multiple languages.
          </span>
        </div>
        <div class="settings-field">
          <label>Custom vocabulary</label>
          <div style="display: block; width: 100%; min-width: 0;">
            <textarea data-key="whisper.initial_prompt" id="vocab-input" maxlength="8000" rows="6"
              style="resize: vertical; min-height: 130px; font-size: 0.9286rem; padding: 8px; width: 100%; box-sizing: border-box; display: block;"
              placeholder="Names, jargon, acronyms…">${escapeHtml(this.config.whisper.initial_prompt ?? "")}</textarea>
            <div style="display: flex; justify-content: space-between; align-items: baseline; gap: 12px; margin-top: 4px;">
              <span style="${HELP}">
                Names, jargon, and acronyms the transcriber keeps mis-hearing — list them here and Whisper will lean toward them
                (e.g. <code>Phoneme, pyannote, WebView2</code>). Sent as the prompt to <b>Whisper-based</b> providers (the local
                <code>whisper.cpp</code> server, OpenAI, Groq, and Custom endpoints); capped at Whisper's <b>~224-token</b> prompt
                limit, counted live below. Deepgram, AssemblyAI, and ElevenLabs ignore it for now.
              </span>
              <span id="vocab-count" style="font-size: 0.7143rem; color: var(--fg-faded); white-space: nowrap; flex-shrink: 0;"></span>
            </div>
          </div>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    // Live TOKEN counter for the custom-vocabulary box, counted with Whisper's
    // own BPE so the limit matches reality (dense jargon tokenizes much denser
    // than the old 900-char proxy assumed). Hard-capped at VOCAB_MAX_TOKENS: any
    // edit/paste over the limit is trimmed back to the first 224 tokens.
    const vocabInput = container.querySelector<HTMLTextAreaElement>("#vocab-input");
    const vocabCount = container.querySelector<HTMLElement>("#vocab-count");
    let vocabTok: VocabTokenizer | null = null;
    const updateVocabCount = () => {
      if (!vocabInput || !vocabCount) return;
      // Coarse char backstop runs even before the tokenizer loads.
      if (vocabInput.value.length > VOCAB_CHAR_BACKSTOP) {
        vocabInput.value = vocabInput.value.slice(0, VOCAB_CHAR_BACKSTOP);
        this.config.whisper.initial_prompt = vocabInput.value;
      }
      if (!vocabTok) {
        // Tokenizer still loading — show a neutral placeholder, no token cap yet.
        vocabCount.textContent = "… tokens";
        vocabCount.style.color = "var(--fg-faded)";
        return;
      }
      let tokens = vocabTok.encode(vocabInput.value);
      if (tokens.length > VOCAB_MAX_TOKENS) {
        // Hard cap: keep the first 224 tokens, decode back to text.
        vocabInput.value = vocabTok.decode(tokens.slice(0, VOCAB_MAX_TOKENS));
        this.config.whisper.initial_prompt = vocabInput.value;
        tokens = vocabTok.encode(vocabInput.value);
      }
      const n = tokens.length;
      vocabCount.textContent = `${n} / ${VOCAB_MAX_TOKENS} tokens`;
      vocabCount.style.color =
        n >= VOCAB_MAX_TOKENS ? "var(--err)" : n > VOCAB_MAX_TOKENS - 25 ? "var(--warn)" : "var(--fg-faded)";
    };
    vocabInput?.addEventListener("input", updateVocabCount);
    updateVocabCount();
    void loadVocabTokenizer().then((m) => { vocabTok = m; updateVocabCount(); });

    // Curated STT model dropdown (+ "Other…" free-text) for cloud providers,
    // re-mounted whenever the provider changes so the list matches it.
    const mountWhisperModel = () => {
      const host = container.querySelector<HTMLElement>("#whisper-model-host");
      if (!host) return;
      mountModelField(host, {
        mode: "curated",
        getProvider: () => this.config.whisper.provider ?? "",
        getApiUrl: () => this.config.whisper.api_url ?? "",
        getApiKey: () => this.config.whisper.api_key ?? "",
        getModel: () => this.config.whisper.model ?? "",
        setModel: (m) => { this.config.whisper.model = m; },
        curated: () => curatedSttModels(this.config.whisper.provider ?? ""),
        curatedRich: () => curatedTranscriptionModels(this.config.whisper.provider ?? ""),
      });
    };

    // Local providers keep the file cards + external-server machinery; cloud
    // providers get the shared model field with that provider's suggestions.
    const applyProviderVisibility = (provider: string) => {
      const isLocal = provider === "local";
      container.querySelector<HTMLElement>("#whisper-local")!.style.display = isLocal
        ? ""
        : "none";
      container.querySelector<HTMLElement>("#whisper-cloud")!.style.display = isLocal
        ? "none"
        : "";
      if (isLocal) return;

      // provider metadata is from the shared STT catalog, not user input.
      const { model: defaultModel } = sttMeta(provider);
      const modelHelp = container.querySelector<HTMLElement>("#cloud-model-help");
      if (modelHelp)
        modelHelp.textContent = `Leave blank to use the provider default (${defaultModel}).`;
      mountWhisperModel();
    };

    // The provider/key/endpoint/Test UI is the shared connection block. It
    // reads and writes the same `[whisper]` keys the old controls did —
    // provider kind, api_url, api_key — so configs round-trip untouched. The
    // local mode/port/external_url machinery stays out of its reach: the
    // block only resolves the URL its Test button probes via testUrl().
    const connHost = container.querySelector<HTMLElement>("#whisper-connection")!;
    mountConnectionField(connHost, {
      catalog: "stt",
      getKind: () => String(this.config.whisper.provider ?? "local"),
      setKind: (k: string) => { this.config.whisper.provider = k; },
      getApiUrl: () => String(this.config.whisper.api_url ?? ""),
      setApiUrl: (u: string) => { this.config.whisper.api_url = u; },
      getApiKey: () => String(this.config.whisper.api_key ?? ""),
      setApiKey: (k: string) => { this.config.whisper.api_key = k; },
      onProviderChanged: () => applyProviderVisibility(this.config.whisper.provider ?? "local"),
      resolveTestUrl: () => this.testUrl(),
    });
    applyProviderVisibility(this.config.whisper.provider ?? "local");

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
        input.dispatchEvent(new Event("change", { bubbles: true }));
        this.config.whisper.model_path = path;
        void this.fetchHardwareAndModels(); // Re-render selected state
      }
    });
  }
}
