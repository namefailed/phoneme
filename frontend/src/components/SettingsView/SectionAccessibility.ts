import { renderField, bindFieldEvents } from "./form";

export class SectionAccessibility {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    // Ensure nested object exists in the loaded config to prevent undefined errors
    if (!config.llm_post_process) {
      config.llm_post_process = {
        enabled: false,
        provider: "none",
        api_key: "",
        model: "llama3",
        prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone."
      };
    }

    container.innerHTML = `
      <div class="settings-section">
        <h3>AI Post-Processing</h3>
        <p style="font-size: 12px; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Automatically edit, reformat, and clean up your transcript using a local or remote LLM.
        </p>

        <div style="background-color: var(--bg-inset); padding: 12px; border-radius: 6px; border: 1px solid var(--border-color); margin-bottom: 16px;">
          <strong style="display: block; font-size: 13px; margin-bottom: 6px; color: var(--fg-default);">How to use this for free (Offline):</strong>
          <ol style="margin: 0; padding-left: 20px; font-size: 12px; color: var(--fg-muted); line-height: 1.5;">
            <li>Download and install <a href="#" onclick="require('@tauri-apps/api/core').invoke('open_file', { path: 'https://ollama.com/download' })" style="color: var(--accent); text-decoration: none;">Ollama</a>.</li>
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
                { value: "openai", label: "OpenAI-Compatible Endpoint" }
              ]
            },
            config.llm_post_process.provider || "none",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="ollama" style="display: none;">
          <label>Model Name</label>
          <div style="flex: 1;">${renderField(
            { key: "llm_post_process.model", label: "", kind: "text" },
            config.llm_post_process.model || "llama3",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="ollama" style="display: none;">
          <label>Ollama API URL</label>
          <div style="flex: 1;">${renderField(
            { key: "llm_post_process.api_url", label: "", kind: "text" },
            config.llm_post_process.api_url || "http://127.0.0.1:11434/api/generate",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="openai" style="display: none;">
          <label>OpenAI Model</label>
          <div style="flex: 1;">${renderField(
            { key: "llm_post_process.model", label: "", kind: "text" },
            config.llm_post_process.model || "gpt-4o",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="openai" style="display: none;">
          <label>API Key</label>
          <div style="flex: 1;">${renderField(
            { key: "llm_post_process.api_key", label: "", kind: "text", type: "password" },
            config.llm_post_process.api_key || "",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="openai" style="display: none;">
          <label>OpenAI API URL</label>
          <div style="flex: 1;">${renderField(
            { key: "llm_post_process.api_url", label: "", kind: "text" },
            config.llm_post_process.api_url || "https://api.openai.com/v1/chat/completions",
          )}</div>
        </div>

        <div class="settings-field" style="display: flex; flex-direction: column; align-items: stretch; gap: 8px; border-bottom: none; padding-bottom: 0;">
          <style>
            textarea[data-key="llm_post_process.prompt"] {
              max-width: 100% !important;
              min-height: 250px !important;
            }
          </style>
          <label style="margin-bottom: 0;">Instructions for the AI</label>
          
          <div style="width: 100%; display: flex; gap: 8px; margin-bottom: 4px; align-items: center;">
            <select id="prompt-preset-select" style="flex: 1; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 4px 8px; font-size: 12px; color: var(--fg-default); outline: none; cursor: pointer;">
              <option value="">-- Choose a Default Preset --</option>
              <option value="Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.">Clean up audio (Default)</option>
              <option value="Fix grammar and punctuation only. Keep the exact words and meaning intact. Do not summarize.">Grammar & Punctuation only</option>
              <option value="Format the transcript as a bulleted list of key takeaways and action items.">Extract action items</option>
              <option value="Summarize the core message of this transcript in 2-3 sentences.">Summarize</option>
              <option value="Rewrite this transcript into a professional, polished email draft.">Write an email</option>
              <option value="Translate this transcript into clear, fluent English.">Translate to English</option>
              <option value="Format this transcript into a structured markdown document with clear headings, bullet points, and bolded key terms.">Structured Markdown Notes</option>
              <option value="I have a speech impediment that causes me to stutter and repeat sounds. Carefully clean up the transcript so it flows perfectly, removing any dysfluency while preserving my intended meaning. Reply ONLY with the cleaned text.">Dysfluency & Stuttering Assist</option>
              <option value="Format this raw transcript into a clean, professional journal entry or meeting note. Use bullet points or headings if appropriate. Output ONLY the formatted notes and absolutely no conversational filler.">Professional Notes & Journal</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); white-space: nowrap;">Select a preset to auto-fill</span>
          </div>

          <div style="width: 100%;">${renderField(
            { key: "llm_post_process.prompt", label: "", kind: "textarea" },
            config.llm_post_process.prompt || "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.",
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); line-height: 1.4;">
            Instructions for the AI to follow when editing the transcript. Make sure to instruct the AI to only output the final text.
          </span>
        </div>
      </div>
    `;

    bindFieldEvents(container, config);

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

    const providerSelect = container.querySelector<HTMLSelectElement>("[data-key='llm_post_process.provider']");
    const providerSettings = container.querySelectorAll<HTMLElement>(".provider-settings");

    const updateProviderVisibility = () => {
      const provider = providerSelect?.value || "none";
      providerSettings.forEach(el => {
        if (el.dataset.provider === provider) {
          el.style.display = "flex";
        } else {
          el.style.display = "none";
        }
      });
    };

    if (providerSelect) {
      providerSelect.addEventListener("change", updateProviderVisibility);
      updateProviderVisibility(); // Initial run
    }
  }
}
