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
        api_url: "",
        model: "llama3.2:3b",
        prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.",
        timeout_secs: 30
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
                { value: "openai", label: "OpenAI-Compatible Endpoint" },
                { value: "groq", label: "Groq (cloud)" },
                { value: "anthropic", label: "Anthropic Claude (cloud)" }
              ]
            },
            config.llm_post_process.provider || "none",
          )}</div>
        </div>

        <div class="settings-field provider-settings" data-provider="ollama" style="display: none;">
          <label>Model Name</label>
          <div>${renderField(
            { key: "llm_post_process.model", label: "", kind: "text" },
            config.llm_post_process.model || "llama3.2:3b",
          )}</div>
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
          <div>${renderField(
            { key: "llm_post_process.model", label: "", kind: "text" },
            config.llm_post_process.model || "gpt-4o",
          )}</div>
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
          <div>${renderField(
            { key: "llm_post_process.model", label: "", kind: "text" },
            config.llm_post_process.model || "llama-3.1-8b-instant",
          )}</div>
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
          <div>${renderField(
            { key: "llm_post_process.model", label: "", kind: "text" },
            config.llm_post_process.model || "claude-3-5-haiku-latest",
          )}</div>
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
    };

    if (providerSelect) {
      providerSelect.addEventListener("change", updateProviderVisibility);
      updateProviderVisibility(); // Initial run
    }
  }
}
