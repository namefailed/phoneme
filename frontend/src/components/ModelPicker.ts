import "./modal.css";
import "./model-picker.css";
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../utils/toast";
import { escapeAttr } from "../utils/format";

/**
 * Model-picker modal — lets the user quickly switch the transcription provider
 * /model and the AI post-processing provider/model without diving into the full
 * Settings screen. Loads the current config via `read_config` and persists via
 * `write_config`, mirroring the Settings sections.
 *
 * Reachable from a button in Settings and from the Re-transcribe caret.
 */

type ProviderOption = { value: string; label: string };

/** Named presets that map onto an existing OpenAI-compatible provider and
 *  prefill the endpoint + a sensible default model. Frontend-only — the Rust
 *  backend already speaks OpenAI-compatible endpoints. */
type Preset = {
  /** dropdown id, e.g. "preset:gemini" */
  id: string;
  label: string;
  /** underlying real provider the backend understands */
  provider: string;
  apiUrl: string;
  model: string;
};

// Post-processing (LLM) presets → the existing "openai" (OpenAI-Compatible
// Endpoint) provider, full chat-completions URL.
const LLM_PRESETS: Preset[] = [
  { id: "preset:gemini", label: "Google Gemini", provider: "openai", apiUrl: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions", model: "gemini-flash-latest" },
  { id: "preset:mistral", label: "Mistral", provider: "openai", apiUrl: "https://api.mistral.ai/v1/chat/completions", model: "mistral-small-latest" },
  { id: "preset:deepseek", label: "DeepSeek", provider: "openai", apiUrl: "https://api.deepseek.com/v1/chat/completions", model: "deepseek-chat" },
  { id: "preset:openrouter", label: "OpenRouter", provider: "openai", apiUrl: "https://openrouter.ai/api/v1/chat/completions", model: "meta-llama/llama-3.3-70b-instruct:free" },
  { id: "preset:together", label: "Together", provider: "openai", apiUrl: "https://api.together.xyz/v1/chat/completions", model: "meta-llama/Llama-3.3-70B-Instruct-Turbo" },
  { id: "preset:xai", label: "xAI / Grok", provider: "openai", apiUrl: "https://api.x.ai/v1/chat/completions", model: "grok-2-latest" },
  { id: "preset:cerebras", label: "Cerebras", provider: "openai", apiUrl: "https://api.cerebras.ai/v1/chat/completions", model: "llama-3.3-70b" },
  { id: "preset:lmstudio", label: "LM Studio (local)", provider: "openai", apiUrl: "http://localhost:1234/v1/chat/completions", model: "" },
];

// Transcription presets → the existing "custom" (OpenAI-compatible endpoint)
// provider, base URL (the backend appends /v1/audio/transcriptions).
const STT_PRESETS: Preset[] = [
  { id: "preset:fireworks", label: "Fireworks", provider: "custom", apiUrl: "https://api.fireworks.ai/inference", model: "whisper-v3" },
];

const STT_PROVIDERS: ProviderOption[] = [
  { value: "local", label: "Local — whisper.cpp (offline, default)" },
  { value: "openai", label: "OpenAI (cloud)" },
  { value: "groq", label: "Groq (cloud)" },
  { value: "deepgram", label: "Deepgram (cloud)" },
  { value: "assemblyai", label: "AssemblyAI (cloud)" },
  { value: "elevenlabs", label: "ElevenLabs Scribe (cloud)" },
  { value: "custom", label: "Custom (OpenAI-compatible endpoint)" },
];

const LLM_PROVIDERS: ProviderOption[] = [
  { value: "none", label: "None" },
  { value: "ollama", label: "Local Ollama (http://127.0.0.1:11434)" },
  { value: "openai", label: "OpenAI-Compatible Endpoint" },
  { value: "groq", label: "Groq (cloud)" },
  { value: "anthropic", label: "Anthropic Claude (cloud)" },
];

function buildProviderOptions(
  providers: ProviderOption[],
  presets: Preset[],
  selected: string,
): string {
  const real = providers
    .map(
      (p) =>
        `<option value="${escapeAttr(p.value)}" ${p.value === selected ? "selected" : ""}>${p.label}</option>`,
    )
    .join("");
  const presetOpts = presets
    .map((p) => `<option value="${escapeAttr(p.id)}">${p.label}</option>`)
    .join("");
  return `${real}<optgroup label="Presets">${presetOpts}</optgroup>`;
}

/**
 * Opens the model picker. Resolves `true` if the user saved, `false` if
 * cancelled.
 *
 * When `anchor` is supplied (e.g. the "Re-transcribe ▾" caret) the picker is
 * rendered as a dropdown positioned directly beneath that element instead of a
 * centered modal. Without an anchor (e.g. the Settings button) it stays a
 * centered modal.
 */
export function openModelPicker(
  initialTab: "transcription" | "postprocessing" = "transcription",
  anchor?: HTMLElement,
): Promise<boolean> {
  return run(initialTab, anchor);
}

async function run(
  initialTab: "transcription" | "postprocessing",
  anchor?: HTMLElement,
): Promise<boolean> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let config: any;
  try {
    config = await invoke("read_config");
  } catch (e) {
    showToast(`Failed to load config: ${e}`, "error");
    return false;
  }

  // Ensure the nested objects exist so reads/writes don't throw.
  if (!config.whisper) config.whisper = {};
  if (!config.llm_post_process) config.llm_post_process = {};

  const w = config.whisper;
  const l = config.llm_post_process;

  return await new Promise<boolean>((resolve) => {

  const overlay = document.createElement("div");
  overlay.className = anchor ? "modal-overlay mp-anchored" : "modal-overlay";
  overlay.innerHTML = `
    <div class="modal-dialog mp-dialog" role="dialog" aria-modal="true" aria-labelledby="mp-title">
      <div class="modal-header">
        <h3 class="modal-title" id="mp-title">Choose Models</h3>
      </div>

      <div class="mp-tabs" role="tablist">
        <button class="mp-tab" data-tab="transcription" role="tab">Transcription</button>
        <button class="mp-tab" data-tab="postprocessing" role="tab">Post-processing</button>
      </div>

      <div class="mp-panel" data-panel="transcription">
        <label class="mp-label" for="mp-stt-provider">Provider</label>
        <select id="mp-stt-provider" class="mp-input">
          ${buildProviderOptions(STT_PROVIDERS, STT_PRESETS, String(w.provider ?? "local"))}
        </select>

        <div class="mp-row" data-stt-cloud>
          <label class="mp-label" for="mp-stt-key">API key</label>
          <input id="mp-stt-key" class="mp-input" type="password" value="${escapeAttr(String(w.api_key ?? ""))}" />

          <label class="mp-label" for="mp-stt-url">API URL (optional)</label>
          <input id="mp-stt-url" class="mp-input" type="text" value="${escapeAttr(String(w.api_url ?? ""))}" />

          <label class="mp-label" for="mp-stt-model">Model</label>
          <input id="mp-stt-model" class="mp-input" type="text" value="${escapeAttr(String(w.model ?? ""))}" placeholder="Leave blank for provider default" />
        </div>

        <p class="mp-hint" data-stt-local-hint>Where your audio is transcribed. <b>Local</b> stays on your machine and uses the bundled model from full Settings; cloud options upload audio to a third-party API.</p>
      </div>

      <div class="mp-panel" data-panel="postprocessing" hidden>
        <label class="mp-label" for="mp-llm-provider">Provider</label>
        <select id="mp-llm-provider" class="mp-input">
          ${buildProviderOptions(LLM_PROVIDERS, LLM_PRESETS, String(l.provider ?? "none"))}
        </select>

        <div class="mp-row" data-llm-cloud>
          <label class="mp-label" for="mp-llm-key">API key</label>
          <input id="mp-llm-key" class="mp-input" type="password" value="${escapeAttr(String(l.api_key ?? ""))}" />

          <label class="mp-label" for="mp-llm-url">API URL (optional)</label>
          <input id="mp-llm-url" class="mp-input" type="text" value="${escapeAttr(String(l.api_url ?? ""))}" />
        </div>

        <label class="mp-label" for="mp-llm-model">Model</label>
        <input id="mp-llm-model" class="mp-input" type="text" value="${escapeAttr(String(l.model ?? ""))}" placeholder="e.g. llama3.2:3b" />
        <p class="mp-hint">Optional LLM clean-up of your transcript. <b>None</b> disables it; <b>Local Ollama</b> keeps everything offline.</p>
      </div>

      <div class="modal-actions">
        <button id="mp-cancel" class="modal-btn">Cancel</button>
        <button id="mp-save" class="modal-btn modal-btn-primary">Save</button>
      </div>
    </div>
  `;

  document.body.appendChild(overlay);

  // When anchored to a trigger (the Re-transcribe caret), position the dialog
  // as a dropdown directly beneath it, clamped to stay within the viewport.
  if (anchor) {
    const dialog = overlay.querySelector<HTMLElement>(".mp-dialog")!;
    const rect = anchor.getBoundingClientRect();
    const width = dialog.offsetWidth;
    const margin = 8;
    let left = rect.left;
    if (left + width + margin > window.innerWidth) {
      left = Math.max(margin, window.innerWidth - width - margin);
    }
    let top = rect.bottom + 4;
    const height = dialog.offsetHeight;
    // If it would overflow the bottom, flip above the anchor when there's room.
    if (top + height + margin > window.innerHeight && rect.top - height - 4 > margin) {
      top = rect.top - height - 4;
    }
    dialog.style.top = `${Math.max(margin, top)}px`;
    dialog.style.left = `${left}px`;
  }

  const sttProvider = overlay.querySelector<HTMLSelectElement>("#mp-stt-provider")!;
  const sttUrl = overlay.querySelector<HTMLInputElement>("#mp-stt-url")!;
  const sttModel = overlay.querySelector<HTMLInputElement>("#mp-stt-model")!;
  const sttKey = overlay.querySelector<HTMLInputElement>("#mp-stt-key")!;
  const sttCloud = overlay.querySelector<HTMLElement>("[data-stt-cloud]")!;

  const llmProvider = overlay.querySelector<HTMLSelectElement>("#mp-llm-provider")!;
  const llmUrl = overlay.querySelector<HTMLInputElement>("#mp-llm-url")!;
  const llmModel = overlay.querySelector<HTMLInputElement>("#mp-llm-model")!;
  const llmKey = overlay.querySelector<HTMLInputElement>("#mp-llm-key")!;
  const llmCloud = overlay.querySelector<HTMLElement>("[data-llm-cloud]")!;

  // Track the real provider chosen so a preset selection resolves to a backend
  // provider while leaving the dropdown showing the preset label was applied.
  let sttRealProvider = String(w.provider ?? "local");
  let llmRealProvider = String(l.provider ?? "none");

  const updateSttCloud = () => {
    sttCloud.style.display = sttRealProvider === "local" ? "none" : "";
  };
  const updateLlmCloud = () => {
    const isCloud =
      llmRealProvider === "openai" ||
      llmRealProvider === "groq" ||
      llmRealProvider === "anthropic";
    llmCloud.style.display = isCloud ? "" : "none";
  };

  sttProvider.addEventListener("change", () => {
    const v = sttProvider.value;
    const preset = STT_PRESETS.find((p) => p.id === v);
    if (preset) {
      sttRealProvider = preset.provider;
      sttUrl.value = preset.apiUrl;
      sttModel.value = preset.model;
    } else {
      sttRealProvider = v;
    }
    updateSttCloud();
  });

  llmProvider.addEventListener("change", () => {
    const v = llmProvider.value;
    const preset = LLM_PRESETS.find((p) => p.id === v);
    if (preset) {
      llmRealProvider = preset.provider;
      llmUrl.value = preset.apiUrl;
      llmModel.value = preset.model;
    } else {
      llmRealProvider = v;
    }
    updateLlmCloud();
  });

  updateSttCloud();
  updateLlmCloud();

  // Tabs
  const tabs = overlay.querySelectorAll<HTMLButtonElement>(".mp-tab");
  const panels = overlay.querySelectorAll<HTMLElement>(".mp-panel");
  const activateTab = (name: string) => {
    tabs.forEach((t) => t.classList.toggle("active", t.dataset.tab === name));
    panels.forEach((p) => {
      p.hidden = p.dataset.panel !== name;
    });
  };
  tabs.forEach((t) =>
    t.addEventListener("click", () => activateTab(t.dataset.tab!)),
  );
  activateTab(initialTab);

  const close = (saved: boolean) => {
    overlay.remove();
    document.removeEventListener("keydown", keyHandler);
    resolve(saved);
  };
  const keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape") close(false);
  };
  document.addEventListener("keydown", keyHandler);
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) close(false);
  });
  overlay.querySelector("#mp-cancel")!.addEventListener("click", () => close(false));

  overlay.querySelector("#mp-save")!.addEventListener("click", async () => {
    config.whisper.provider = sttRealProvider;
    config.whisper.model = sttModel.value.trim();
    config.whisper.api_key = sttKey.value;
    config.whisper.api_url = sttUrl.value.trim();

    config.llm_post_process.provider = llmRealProvider;
    config.llm_post_process.model = llmModel.value.trim();
    config.llm_post_process.api_key = llmKey.value;
    config.llm_post_process.api_url = llmUrl.value.trim();
    // Keep the enabled flag in sync: a real LLM provider means it's on.
    config.llm_post_process.enabled = llmRealProvider !== "none";

    try {
      await invoke("write_config", { config });
      window.dispatchEvent(new CustomEvent("config:saved", { detail: config }));
      showToast("Models saved", "success");
      close(true);
    } catch (e) {
      showToast(`Save failed: ${e}`, "error");
    }
  });

    (overlay.querySelector("#mp-cancel") as HTMLButtonElement)?.focus();
  });
}
