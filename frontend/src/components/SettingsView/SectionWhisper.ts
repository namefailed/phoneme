import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

const HELP =
  "font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;";

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
          <label>Provider</label>
          <div>${renderField(
            {
              key: "whisper.provider",
              label: "",
              kind: "select",
              options: [
                { value: "local", label: "Local — whisper.cpp (offline, default)" },
                { value: "openai", label: "OpenAI (cloud)" },
                { value: "groq", label: "Groq (cloud)" },
              ],
            },
            this.config.whisper.provider ?? "local",
          )}</div>
          <span style="${HELP}">
            Where your audio is transcribed. <b>Local</b> runs entirely on your machine; cloud options send audio to a third-party API.
          </span>
        </div>

        <div id="whisper-cloud" style="display:none">
          <div
            id="cloud-warning"
            style="border:1px solid var(--err); border-radius:6px; padding:8px 10px; margin-bottom:14px; font-size:12px; line-height:1.45;"
          ></div>
          <div class="settings-field long-input">
            <label>API key</label>
            <div>${renderField(
              { key: "whisper.api_key", label: "", kind: "text", type: "password" },
              this.config.whisper.api_key ?? "",
            )}</div>
            <span style="${HELP}">
              Your <span id="cloud-name">cloud</span> API key. Stored locally in your <code>config.toml</code>; never sent anywhere except the provider.
            </span>
          </div>
          <div class="settings-field long-input">
            <label>Model</label>
            <div>${renderField(
              { key: "whisper.model", label: "", kind: "text" },
              this.config.whisper.model ?? "",
            )}</div>
            <span style="${HELP}" id="cloud-model-help">
              Leave blank to use the provider default.
            </span>
          </div>
          <div class="settings-field long-input">
            <label>API URL (optional)</label>
            <div>${renderField(
              { key: "whisper.api_url", label: "", kind: "text" },
              this.config.whisper.api_url ?? "",
            )}</div>
            <span style="${HELP}">
              Optional. Override the endpoint for a proxy or OpenAI-compatible gateway. Leave blank for the provider default.
            </span>
          </div>
        </div>

        <div id="whisper-local">
          <div class="settings-field long-input">
            <label>External URL</label>
            <div>
              ${renderField(
                { key: "whisper.external_url", label: "", kind: "text" },
                this.config.whisper.external_url,
              )}
              <button class="inline-button" id="test-whisper">Test</button>
              <div class="test-result" id="whisper-result" style="display:none"></div>
            </div>
            <span style="${HELP}">
              The endpoint to send audio to when using <b>External</b> mode. Must be a Whisper-compatible API (e.g., <code>http://127.0.0.1:8080/inference</code>).
            </span>
          </div>
          <div class="settings-field long-input">
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
            <span style="${HELP}">
              The absolute path to a GGML <code>.bin</code> model file. Used when running the <b>Bundled model</b>. Click <b>Download Default</b> to fetch the <code>ggml-base.en.bin</code> model automatically.
            </span>
          </div>
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
      </div>
    `;
    bindFieldEvents(container, this.config);

    // Show local vs cloud settings based on the selected provider.
    const applyProviderVisibility = (provider: string) => {
      const isLocal = provider !== "openai" && provider !== "groq";
      container.querySelector<HTMLElement>("#whisper-local")!.style.display = isLocal
        ? ""
        : "none";
      container.querySelector<HTMLElement>("#whisper-cloud")!.style.display = isLocal
        ? "none"
        : "";
      if (isLocal) return;

      // provider/host/model are from a fixed set, not user input — safe in innerHTML.
      const name = provider === "groq" ? "Groq" : "OpenAI";
      const host = provider === "groq" ? "api.groq.com" : "api.openai.com";
      const defaultModel = provider === "groq" ? "whisper-large-v3" : "whisper-1";
      container.querySelector<HTMLElement>("#cloud-warning")!.innerHTML =
        `⚠️ <b>Cloud transcription.</b> Selecting ${name} uploads your recorded audio to ` +
        `${host} for processing — your audio leaves your machine. Switch back to ` +
        `<b>Local</b> to keep everything offline.`;
      const cloudName = container.querySelector<HTMLElement>("#cloud-name");
      if (cloudName) cloudName.textContent = name;
      const modelHelp = container.querySelector<HTMLElement>("#cloud-model-help");
      if (modelHelp)
        modelHelp.textContent = `Leave blank to use the provider default (${defaultModel}).`;
    };

    const providerSelect = container.querySelector<HTMLSelectElement>(
      `[data-key="whisper.provider"]`,
    );
    providerSelect?.addEventListener("change", () =>
      applyProviderVisibility(providerSelect.value),
    );
    applyProviderVisibility(this.config.whisper.provider ?? "local");

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
