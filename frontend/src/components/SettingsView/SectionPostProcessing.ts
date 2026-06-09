import { renderField, bindFieldEvents } from "./form";
import { mountModelField } from "./modelField";
import {
  LOCAL_LLM_PRESETS,
  CLOUD_LLM_PRESETS,
  findLlmPreset,
  type LlmPreset,
} from "../../services/llmProviders";

/** Grouped <option> list (Local / Cloud) for a provider-preset dropdown. */
function presetOptionsHtml(): string {
  const opt = (p: LlmPreset) => `<option value="${p.id}">${p.label}${p.needsKey ? "" : ""}</option>`;
  return `
    <optgroup label="Local / offline">${LOCAL_LLM_PRESETS.map(opt).join("")}</optgroup>
    <optgroup label="Cloud (API key)">${CLOUD_LLM_PRESETS.map(opt).join("")}</optgroup>`;
}

export class SectionPostProcessing {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    // Ensure nested object exists in the loaded config to prevent undefined errors
    if (!config.llm_post_process) {
      config.llm_post_process = {
        enabled: false,
        provider: "none",
        api_key: "",
        api_url: "",
        model: "llama3.2:3b",
        prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.",
        timeout_secs: 30
      };
    }

    // Auto-summary settings. Each provider field falls back to the cleanup
    // connection when left blank, so summaries can run on a fully independent
    // provider+model or just reuse the post-processing provider above.
    if (!config.summary) {
      config.summary = {
        auto: false,
        provider: "",
        api_key: "",
        api_url: "",
        model: "",
        prompt: "Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble.",
      };
    }

    // State for provider models
    const providerModels: Record<string, string[]> = { ollama: [], openai: [], groq: [], anthropic: [] };
    const fetchingModels: Record<string, boolean> = { ollama: false, openai: false, groq: false, anthropic: false };

    const fetchProviderModels = async (provider: string) => {
      fetchingModels[provider] = true;
      updateProviderSelect(provider);
      try {
        let urlStr = config.llm_post_process.api_url || "";
        const apiKey = config.llm_post_process.api_key || "";
        let endpoint = "";
        let headers: Record<string, string> = {};
        
        if (provider === "ollama") {
          if (!urlStr) urlStr = "http://127.0.0.1:11434/api/generate";
          const url = new URL(urlStr);
          endpoint = `${url.protocol}//${url.host}/api/tags`;
        } else if (provider === "openai" || provider === "groq") {
          if (!urlStr) {
            urlStr = provider === "openai" 
              ? "https://api.openai.com/v1/chat/completions" 
              : "https://api.groq.com/openai/v1/chat/completions";
          }
          const url = new URL(urlStr);
          let path = url.pathname;
          if (path.endsWith("/chat/completions")) {
            path = path.replace("/chat/completions", "/models");
          } else if (!path.endsWith("/models")) {
            path = path.endsWith("/") ? path + "models" : path + "/models";
          }
          endpoint = `${url.protocol}//${url.host}${path}`;
          headers["Authorization"] = `Bearer ${apiKey}`;
        } else if (provider === "anthropic") {
          if (!urlStr) urlStr = "https://api.anthropic.com/v1/messages";
          const url = new URL(urlStr);
          endpoint = `${url.protocol}//${url.host}/v1/models`;
          headers["x-api-key"] = apiKey;
          headers["anthropic-version"] = "2023-06-01";
        }

        const res = await fetch(endpoint, { headers });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        
        if (provider === "ollama") {
          providerModels[provider] = (data.models || []).map((m: any) => m.name);
        } else if (provider === "openai" || provider === "groq" || provider === "anthropic") {
          providerModels[provider] = (data.data || []).map((m: any) => m.id);
        }
      } catch (e) {
        console.warn(`Failed to fetch ${provider} models:`, e);
        providerModels[provider] = [];
      } finally {
        fetchingModels[provider] = false;
        updateProviderSelect(provider);
      }
    };

    const updateProviderSelect = (provider: string) => {
      const select = container.querySelector<HTMLSelectElement>(`#${provider}-model-select`);
      if (!select) return;

      const currentModel = config.llm_post_process.model || "";
      select.innerHTML = "";
      
      if (fetchingModels[provider]) {
        const option = document.createElement("option");
        option.disabled = true;
        option.textContent = "Loading models...";
        select.appendChild(option);
      } else if (providerModels[provider].length === 0) {
        const option = document.createElement("option");
        option.value = "";
        option.textContent = "No models found — click Refresh";
        select.appendChild(option);
      } else {
        providerModels[provider].forEach(m => {
          const option = document.createElement("option");
          option.value = m;
          option.textContent = m;
          if (m === currentModel) option.selected = true;
          select.appendChild(option);
        });
      }
      
      // Ensure current model is shown even if not in list
      if (currentModel && !providerModels[provider].includes(currentModel)) {
        const option = document.createElement("option");
        option.value = currentModel;
        option.textContent = `${currentModel} (current)`;
        option.selected = true;
        select.appendChild(option);
      }
    };

    container.innerHTML = `
      <div class="settings-section">
        <h3>AI Post-Processing</h3>
        <p style="font-size: 12px; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Automatically edit, reformat, and clean up your transcript using a local or remote LLM.
        </p>

        <div style="background-color: var(--bg-deep); padding: 12px; border-radius: 6px; border: 1px solid var(--border-subtle); margin-bottom: 16px;">
          <strong style="display: block; font-size: 13px; margin-bottom: 6px; color: var(--fg-default);">How to use this for free (Offline):</strong>
          <ol style="margin: 0; padding-left: 20px; font-size: 12px; color: var(--fg-muted); line-height: 1.5;">
            <li>Download and install <a href="#" id="ollama-download-link" style="color: var(--accent); text-decoration: none;">Ollama</a>.</li>
            <li>Open your terminal and run <code>ollama run llama3.2:3b</code>.</li>
            <li>Select <strong>Local Ollama</strong> below and use <code>llama3.2:3b</code> as your Model Name!</li>
          </ol>
        </div>

        <div class="settings-field">
          <label>Enable AI Post-Processing</label>
          <div>${renderField(
            { key: "llm_post_process.enabled", label: "", kind: "checkbox" },
            config.llm_post_process.enabled,
          )}</div>
        </div>

        <div class="settings-field">
          <label>AI Provider</label>
          <div>${renderField(
            {
              key: "llm_post_process.provider",
              label: "",
              kind: "select",
              options: [
                { value: "none", label: "None" },
                { value: "ollama", label: "Local Ollama (http://127.0.0.1:11434)" },
                { value: "openai", label: "OpenAI-Compatible Endpoint" },
                { value: "groq", label: "Groq (cloud)" },
                { value: "anthropic", label: "Anthropic Claude (cloud)" }
              ]
            },
            config.llm_post_process.provider || "none",
          )}</div>
        </div>

        <div class="settings-field">
          <label>Quick preset</label>
          <div>
            <select id="llm-preset-select" style="max-width: 400px;">
              <option value="">— Pick a provider —</option>
              ${presetOptionsHtml()}
            </select>
          </div>
          <span style="font-size: 11px; color: var(--fg-faded); grid-column: 2;">
            One click sets the provider, endpoint, and a default model. Just add your API key (cloud only).
          </span>
        </div>

        <div class="settings-field provider-settings" data-provider="ollama" style="display: none;">
          <label>Model Name</label>
          <div>
            <div style="display: flex; gap: 8px;">
              <div style="flex: 1;">
                <select id="ollama-model-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);"></select>
              </div>
              <button class="inline-button fetch-models-btn" data-provider="ollama" type="button" style="padding: 4px 10px;">Refresh</button>
            </div>
          </div>
        </div>

        <div class="settings-field provider-settings" data-provider="ollama" style="display: none;">
          <label>Ollama API URL</label>
          <div>${renderField(
            { key: "llm_post_process.api_url", label: "", kind: "text" },
            config.llm_post_process.api_url || "http://127.0.0.1:11434/api/generate",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="openai" style="display: none;">
          <label>OpenAI Model</label>
          <div>
            <div style="display: flex; gap: 8px;">
              <div style="flex: 1;">
                <select id="openai-model-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);"></select>
              </div>
              <button class="inline-button fetch-models-btn" data-provider="openai" type="button" style="padding: 4px 10px;">Refresh</button>
            </div>
          </div>
        </div>

        <div class="settings-field provider-settings" data-provider="openai" style="display: none;">
          <label>API Key</label>
          <div>${renderField(
            { key: "llm_post_process.api_key", label: "", kind: "text", type: "password" },
            config.llm_post_process.api_key || "",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="openai" style="display: none;">
          <label>OpenAI API URL</label>
          <div>${renderField(
            { key: "llm_post_process.api_url", label: "", kind: "text" },
            config.llm_post_process.api_url || "https://api.openai.com/v1/chat/completions",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="groq" style="display: none;">
          <label>Groq Model</label>
          <div>
            <div style="display: flex; gap: 8px;">
              <div style="flex: 1;">
                <select id="groq-model-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);"></select>
              </div>
              <button class="inline-button fetch-models-btn" data-provider="groq" type="button" style="padding: 4px 10px;">Refresh</button>
            </div>
          </div>
        </div>
        <div class="settings-field provider-settings" data-provider="groq" style="display: none;">
          <label>API Key</label>
          <div>${renderField(
            { key: "llm_post_process.api_key", label: "", kind: "text", type: "password" },
            config.llm_post_process.api_key || "",
          )}</div>
        </div>
        <div class="settings-field provider-settings" data-provider="groq" style="display: none;">
          <label>API URL (optional)</label>
          <div>${renderField(
            { key: "llm_post_process.api_url", label: "", kind: "text" },
            config.llm_post_process.api_url || "https://api.groq.com/openai/v1/chat/completions",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="anthropic" style="display: none;">
          <label>Claude Model</label>
          <div>
            <div style="display: flex; gap: 8px;">
              <div style="flex: 1;">
                <select id="anthropic-model-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);"></select>
              </div>
              <button class="inline-button fetch-models-btn" data-provider="anthropic" type="button" style="padding: 4px 10px;">Refresh</button>
            </div>
          </div>
        </div>
        <div class="settings-field provider-settings" data-provider="anthropic" style="display: none;">
          <label>API Key</label>
          <div>${renderField(
            { key: "llm_post_process.api_key", label: "", kind: "text", type: "password" },
            config.llm_post_process.api_key || "",
          )}</div>
        </div>
        <div class="settings-field provider-settings" data-provider="anthropic" style="display: none;">
          <label>API URL (optional)</label>
          <div>${renderField(
            { key: "llm_post_process.api_url", label: "", kind: "text" },
            config.llm_post_process.api_url || "https://api.anthropic.com/v1/messages",
          )}</div>
        </div>

        <div id="llm-cloud-note" style="display:none; border:1px solid var(--err); border-radius:6px; padding:8px 10px; margin:4px 0 12px; font-size:12px; line-height:1.45;">
          ⚠️ <b>Cloud post-processing.</b> Your transcript text is sent to this provider's servers for processing. Use <b>Local Ollama</b> to keep everything offline.
        </div>

        <div class="settings-field" id="llm-timeout-field" style="display:none;">
          <label>Request timeout (seconds)</label>
          <div>${renderField(
            { key: "llm_post_process.timeout_secs", label: "", kind: "number" },
            config.llm_post_process.timeout_secs ?? 30,
          )}</div>
        </div>

        <div class="settings-field ai-prompt-field">
          <label>Instructions for the AI</label>
          <div class="ai-prompt-controls">
            <div class="ai-preset-row">
              <select id="prompt-preset-select" class="ai-preset-select">
                <option value="">— Choose a preset —</option>
                <option value="Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.">Clean up audio (Default)</option>
                <option value="Fix grammar and punctuation only. Keep the exact words and meaning intact. Do not summarize.">Grammar &amp; Punctuation only</option>
                <option value="Format the transcript as a bulleted list of key takeaways and action items.">Extract action items</option>
                <option value="Summarize the core message of this transcript in 2-3 sentences.">Summarize</option>
                <option value="Rewrite this transcript into a professional, polished email draft.">Write an email</option>
                <option value="Translate this transcript into clear, fluent English.">Translate to English</option>
                <option value="Format this transcript into a structured markdown document with clear headings, bullet points, and bolded key terms.">Structured Markdown Notes</option>
                <option value="I have a speech impediment that causes me to stutter and repeat sounds. Carefully clean up the transcript so it flows perfectly, removing any dysfluency while preserving my intended meaning. Reply ONLY with the cleaned text.">Dysfluency &amp; Stuttering Assist</option>
                <option value="Format this raw transcript into a clean, professional journal entry or meeting note. Use bullet points or headings if appropriate. Output ONLY the formatted notes and absolutely no conversational filler.">Professional Notes &amp; Journal</option>
              </select>
              <span class="ai-preset-hint">Presets auto-fill the field below</span>
            </div>
            ${renderField(
              { key: "llm_post_process.prompt", label: "", kind: "textarea" },
              config.llm_post_process.prompt || "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.",
            )}
            <span class="settings-help-text">
              Tell the AI how to edit your transcript. Finish with "Reply ONLY with the final text."
            </span>
          </div>
        </div>

        <hr style="border: none; border-top: 1px solid var(--border-subtle); margin: 20px 0 16px;" />

        <h3 style="margin-bottom: 4px;">Auto AI Summary</h3>
        <p style="font-size: 12px; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Generate a short AI summary of each transcript. You can always summarize a single
          recording on demand with the <b>View summary</b> button in its detail view — enabling
          this just runs it automatically as the <b>last step</b> of every recording's pipeline.
          By default summaries reuse your post-processing provider; point them at a different
          provider and model below if you want.
        </p>

        <div class="settings-field">
          <label>Summarize every recording</label>
          <div>${renderField(
            { key: "summary.auto", label: "", kind: "checkbox" },
            config.summary.auto,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); grid-column: 2;">
            When off, summaries are still available on demand per recording.
          </span>
        </div>

        <div class="settings-field">
          <label>Summary provider</label>
          <div>${renderField(
            {
              key: "summary.provider",
              label: "",
              kind: "select",
              options: [
                { value: "", label: "Same as AI Post-Processing (inherit)" },
                { value: "ollama", label: "Local Ollama (http://127.0.0.1:11434)" },
                { value: "openai", label: "OpenAI-Compatible Endpoint" },
                { value: "groq", label: "Groq (cloud)" },
                { value: "anthropic", label: "Anthropic Claude (cloud)" }
              ]
            },
            config.summary.provider || "",
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); grid-column: 2;">
            Summarize with a completely different provider + model than your cleanup step, or leave
            on <b>inherit</b> to reuse the provider configured above.
          </span>
        </div>

        <div class="settings-field summary-preset-row">
          <label>Quick preset</label>
          <div>
            <select id="summary-provider-preset" style="max-width: 400px;">
              <option value="">— Pick a provider —</option>
              ${presetOptionsHtml()}
            </select>
          </div>
          <span style="font-size: 11px; color: var(--fg-faded); grid-column: 2;">
            One click sets the summary provider, endpoint, and a default model. Add your API key (cloud only).
          </span>
        </div>

        <div class="settings-field summary-provider-field summary-needs-key">
          <label>Summary API key</label>
          <div>${renderField(
            { key: "summary.api_key", label: "", kind: "text", type: "password" },
            config.summary.api_key || "",
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); grid-column: 2;">
            Leave blank to reuse the cleanup provider's API key.
          </span>
        </div>

        <div class="settings-field">
          <label>Summary model (optional)</label>
          <div id="summary-model-host"></div>
          <span style="font-size: 11px; color: var(--fg-faded); grid-column: 2;">
            Leave on "Same as cleanup model" to reuse the post-processing model, or pick a different
            one (e.g. a smaller/faster model just for summaries).
          </span>
        </div>

        <div class="settings-field summary-provider-field summary-needs-url">
          <label>Summary API URL</label>
          <div>${renderField(
            { key: "summary.api_url", label: "", kind: "text" },
            config.summary.api_url || "",
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); grid-column: 2;">
            Optional — leave blank for the provider's default endpoint.
          </span>
        </div>

        <div class="settings-field ai-prompt-field">
          <label>Summary instructions</label>
          <div class="ai-prompt-controls">
            <div class="ai-preset-row">
              <select id="summary-preset-select" class="ai-preset-select">
                <option value="">— Choose a preset —</option>
                <option value="Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble.">Bullet-point summary (Default)</option>
                <option value="Summarize the core message of this transcript in 2-3 sentences. Output only the summary.">2-3 sentence summary</option>
                <option value="Extract only the action items and decisions from this transcript as a checklist. Output only the list.">Action items &amp; decisions</option>
                <option value="Write a short paragraph (TL;DR) summarizing what this transcript is about. Output only the paragraph.">TL;DR paragraph</option>
                <option value="Summarize this meeting transcript: list attendees/speakers if identifiable, key discussion points, decisions made, and action items with owners. Output only the structured summary.">Meeting minutes</option>
              </select>
              <span class="ai-preset-hint">Presets auto-fill the field below</span>
            </div>
            ${renderField(
              { key: "summary.prompt", label: "", kind: "textarea" },
              config.summary.prompt || "Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble.",
            )}
            <span class="settings-help-text">
              How the AI should summarize the transcript.
            </span>
          </div>
        </div>
      </div>
    `;

    bindFieldEvents(container, config);

    // Wire up provider model selects
    ["ollama", "openai", "groq", "anthropic"].forEach(provider => {
      const select = container.querySelector<HTMLSelectElement>(`#${provider}-model-select`);
      select?.addEventListener("change", (e) => {
        config.llm_post_process.model = (e.target as HTMLSelectElement).value;
      });

      const refreshBtn = container.querySelector<HTMLButtonElement>(`.fetch-models-btn[data-provider='${provider}']`);
      refreshBtn?.addEventListener("click", () => {
        void fetchProviderModels(provider);
      });
    });

    // Open the Ollama download page in the user's browser. (Was a broken inline
    // `onclick="require(...)"` — `require` doesn't exist in the Vite/ESM bundle,
    // so the link silently threw. Use the shell plugin like the rest of the app.)
    container
      .querySelector<HTMLAnchorElement>("#ollama-download-link")
      ?.addEventListener("click", async (e) => {
        e.preventDefault();
        const { open } = await import("@tauri-apps/plugin-shell");
        await open("https://ollama.com/download").catch(() => {});
      });

    const presetSelect = container.querySelector<HTMLSelectElement>("#prompt-preset-select");
    const promptArea = container.querySelector<HTMLTextAreaElement>("[data-key='llm_post_process.prompt']");
    if (presetSelect && promptArea) {
      presetSelect.addEventListener("change", () => {
        if (presetSelect.value) {
          promptArea.value = presetSelect.value;
          promptArea.dispatchEvent(new Event("input"));
          presetSelect.value = ""; // Reset dropdown to placeholder after applying
        }
      });
    }

    const summaryPresetSelect = container.querySelector<HTMLSelectElement>("#summary-preset-select");
    const summaryPromptArea = container.querySelector<HTMLTextAreaElement>("[data-key='summary.prompt']");
    if (summaryPresetSelect && summaryPromptArea) {
      summaryPresetSelect.addEventListener("change", () => {
        if (summaryPresetSelect.value) {
          summaryPromptArea.value = summaryPresetSelect.value;
          summaryPromptArea.dispatchEvent(new Event("input"));
          summaryPresetSelect.value = "";
        }
      });
    }

    // Summary provider: show/hide the API key + URL fields based on the chosen
    // provider. Inherit ("") hides everything (it reuses the cleanup connection);
    // local Ollama needs only a URL; cloud providers need a key too.
    // Summary model field: live-fetch the effective provider's models (the
    // summary provider, or the inherited cleanup provider when blank).
    const summaryEff = (which: "provider" | "api_url" | "api_key") => {
      const s = (config.summary[which] ?? "").toString().trim();
      return s || (config.llm_post_process[which] ?? "").toString();
    };
    const summaryModelHost = container.querySelector<HTMLElement>("#summary-model-host");
    const mountSummaryModel = () => {
      if (!summaryModelHost) return;
      mountModelField(summaryModelHost, {
        mode: "llm",
        blankLabel: "Same as cleanup model",
        getProvider: () => summaryEff("provider"),
        getApiUrl: () => summaryEff("api_url"),
        getApiKey: () => summaryEff("api_key"),
        getModel: () => config.summary.model || "",
        setModel: (m) => { config.summary.model = m; },
      });
    };
    mountSummaryModel();

    const summaryProviderSelect = container.querySelector<HTMLSelectElement>("[data-key='summary.provider']");
    const updateSummaryProviderVisibility = () => {
      const provider = summaryProviderSelect?.value || "";
      const isCloud = provider === "openai" || provider === "groq" || provider === "anthropic";
      const needsUrl = provider !== "";
      container.querySelectorAll<HTMLElement>(".summary-needs-key").forEach((el) => {
        el.style.display = isCloud ? "grid" : "none";
      });
      container.querySelectorAll<HTMLElement>(".summary-needs-url").forEach((el) => {
        el.style.display = needsUrl ? "grid" : "none";
      });
      const presetRow = container.querySelector<HTMLElement>(".summary-preset-row");
      if (presetRow) presetRow.style.display = "grid";
    };
    if (summaryProviderSelect) {
      summaryProviderSelect.addEventListener("change", () => {
        updateSummaryProviderVisibility();
        mountSummaryModel(); // provider changed → re-fetch its model list
      });
      updateSummaryProviderVisibility();
    }

    // Summary provider presets — map a named entry onto the OpenAI-compatible
    // provider and prefill the endpoint + a default model (mirrors the cleanup
    // presets, but writes to the summary.* keys).
    const summaryProviderPreset = container.querySelector<HTMLSelectElement>("#summary-provider-preset");
    summaryProviderPreset?.addEventListener("change", () => {
      const preset = findLlmPreset(summaryProviderPreset.value);
      if (!preset || !summaryProviderSelect) return;
      // Apply the preset's protocol kind, endpoint, and default model.
      summaryProviderSelect.value = preset.kind;
      summaryProviderSelect.dispatchEvent(new Event("change", { bubbles: true }));
      const urlInput = container.querySelector<HTMLInputElement>(".summary-needs-url [data-key='summary.api_url']");
      if (urlInput) {
        urlInput.value = preset.apiUrl;
        urlInput.dispatchEvent(new Event("input", { bubbles: true }));
      }
      config.summary.model = preset.defaultModel;
      mountSummaryModel(); // re-fetch for the new provider + show its default model
      summaryProviderPreset.value = "";
    });

    const providerSelect = container.querySelector<HTMLSelectElement>("[data-key='llm_post_process.provider']");
    const providerSettings = container.querySelectorAll<HTMLElement>(".provider-settings");

    const updateProviderVisibility = () => {
      const provider = providerSelect?.value || "none";
      providerSettings.forEach(el => {
        if (el.dataset.provider === provider) {
          el.style.display = "grid";
        } else {
          el.style.display = "none";
        }
      });
      // Timeout applies to every active provider; the cloud note only to remote ones.
      const isCloud = provider === "openai" || provider === "groq" || provider === "anthropic";
      const timeoutEl = container.querySelector<HTMLElement>("#llm-timeout-field");
      if (timeoutEl) timeoutEl.style.display = provider === "none" ? "none" : "";
      const cloudNote = container.querySelector<HTMLElement>("#llm-cloud-note");
      if (cloudNote) cloudNote.style.display = isCloud ? "" : "none";
      
      // Fetch models when provider is selected
      if (provider !== "none") {
        void fetchProviderModels(provider);
      }
    };

    if (providerSelect) {
      providerSelect.addEventListener("change", updateProviderVisibility);
      updateProviderVisibility(); // Initial run
    }

    // Provider presets — one click applies a named provider from the shared
    // catalog: its protocol kind, endpoint, and a default model. The user only
    // needs to add an API key (cloud) or have the local server running.
    const llmPresetSelect = container.querySelector<HTMLSelectElement>("#llm-preset-select");
    llmPresetSelect?.addEventListener("change", () => {
      const preset = findLlmPreset(llmPresetSelect.value);
      if (!preset || !providerSelect) return;
      // Write the config directly so the model survives the panel's async model
      // fetch (which repopulates the per-provider <select> on provider change).
      config.llm_post_process.provider = preset.kind;
      config.llm_post_process.api_url = preset.apiUrl;
      config.llm_post_process.model = preset.defaultModel;
      providerSelect.value = preset.kind;
      providerSelect.dispatchEvent(new Event("change", { bubbles: true }));
      // Reflect the endpoint in the now-visible panel's URL input.
      const urlInput = container.querySelector<HTMLInputElement>(
        `.provider-settings[data-provider='${preset.kind}'] [data-key='llm_post_process.api_url']`,
      );
      if (urlInput) {
        urlInput.value = preset.apiUrl;
        urlInput.dispatchEvent(new Event("input", { bubbles: true }));
      }
      // Re-render the model <select> so the preset's default model shows even
      // before a live fetch returns.
      updateProviderSelect(preset.kind);
      llmPresetSelect.value = ""; // reset to placeholder after applying
    });

  }
}
