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
        <h3>Smart Cleanup (AI)</h3>
        <p style="font-size: 12px; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Automatically correct stuttering, accents, lisps, or repetitive words. When enabled, a local or remote AI processes your transcript immediately after transcription.
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
          <label>Enable Smart Cleanup</label>
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

        <div class="settings-field">
          <label>Model Name</label>
          <div>${renderField(
            { key: "llm_post_process.model", label: "", kind: "text" },
            config.llm_post_process.model || "llama3",
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            e.g., <code>llama3</code>, <code>gpt-4o-mini</code>, or <code>llama-3.2-3b-instruct</code>.
          </span>
        </div>

        <div class="settings-field">
          <label>API Key / Bearer Token</label>
          <div>${renderField(
            { key: "llm_post_process.api_key", label: "", kind: "text" },
            config.llm_post_process.api_key || "",
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            Leave blank if using a local provider that doesn't require authentication (like Ollama).
          </span>
        </div>

        <div class="settings-field" style="flex-direction: column; align-items: flex-start; gap: 8px;">
          <label>Instructions for the AI</label>
          <div style="width: 100%;">${renderField(
            { key: "llm_post_process.prompt", label: "", kind: "textarea" },
            config.llm_post_process.prompt || "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.",
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); line-height: 1.4;">
            Instructions for the AI to follow when editing the transcript.
          </span>
        </div>
      </div>
    `;

    bindFieldEvents(container, config);
  }
}
