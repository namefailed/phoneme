import { curatedCleanupModelIds } from "../../data/curatedModels";

/**
 * Auto-tagging settings (Post-Processing tab). The LLM proposes tags for each
 * transcript — preferring tags you already use — and the proposals wait as
 * dashed chips in the recording's tag row until you approve or dismiss them.
 * Nothing is ever applied automatically.
 *
 * Connection fields mirror the Summary section: blank provider/key/URL/model
 * inherit the `[llm_post_process]` (cleanup) connection, so the common case is
 * just flipping the toggle on.
 */
export class SectionAutoTag {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(private container: HTMLElement, private config: any) {
    if (!this.config.auto_tag) {
      this.config.auto_tag = {
        auto: false,
        provider: "",
        api_key: "",
        api_url: "",
        model: "",
        prompt: "",
        max_tags: 5,
      };
    }
    this.render();
  }

  private render() {
    const t = this.config.auto_tag;
    const isCloud = ["openai", "groq", "anthropic"].includes(t.provider);
    const curated = t.provider ? curatedCleanupModelIds(t.provider) : [];

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Auto-Tagging</h3>
        <p style="font-size:12px; color:var(--fg-muted); margin:0 0 4px;">
          Let the AI propose tags for each new transcript (it prefers tags you already use).
          Proposals appear as dashed ✨ chips on the recording — <b>you approve or dismiss
          each one</b>; nothing is tagged automatically. The ✨ Suggest button on a recording
          runs this on demand even when the automatic step is off.
        </p>

        <div class="settings-field">
          <label>Suggest tags automatically</label>
          <div><input type="checkbox" class="toggle-switch" id="at-auto" data-key="auto_tag.auto" ${t.auto ? "checked" : ""} /></div>
        </div>

        <div class="settings-field">
          <label>Provider
            <br><span style="font-size:11px; color:var(--fg-muted); font-weight:normal;">Blank fields inherit your Post-Processing (cleanup) connection.</span>
          </label>
          <div>
            <select id="at-provider" data-key="auto_tag.provider">
              <option value="" ${t.provider === "" ? "selected" : ""}>Same as post-processing</option>
              <option value="ollama" ${t.provider === "ollama" ? "selected" : ""}>Local Ollama</option>
              <option value="openai" ${t.provider === "openai" ? "selected" : ""}>OpenAI-Compatible Endpoint</option>
              <option value="groq" ${t.provider === "groq" ? "selected" : ""}>Groq (cloud)</option>
              <option value="anthropic" ${t.provider === "anthropic" ? "selected" : ""}>Anthropic Claude (cloud)</option>
            </select>
          </div>
        </div>

        ${isCloud ? `
          <div class="settings-field">
            <label>API key</label>
            <div><input type="password" id="at-key" data-key="auto_tag.api_key" value="${escapeAttr(t.api_key ?? "")}" style="width:100%;" /></div>
          </div>
          <div class="settings-field">
            <label>API URL <span style="color:var(--fg-faded); font-weight:normal;">(optional)</span></label>
            <div><input type="text" id="at-url" data-key="auto_tag.api_url" value="${escapeAttr(t.api_url ?? "")}" placeholder="Provider default" style="width:100%;" /></div>
          </div>
        ` : ""}

        ${t.provider ? `
          <div class="settings-field">
            <label>Model <span style="color:var(--fg-faded); font-weight:normal;">(blank = cleanup model)</span></label>
            <div>
              <input type="text" id="at-model" data-key="auto_tag.model" list="at-model-list" value="${escapeAttr(t.model ?? "")}" placeholder="Cleanup model" style="width:100%; max-width:400px;" />
              <datalist id="at-model-list">${curated.map((m) => `<option value="${escapeAttr(m)}"></option>`).join("")}</datalist>
            </div>
          </div>
        ` : ""}

        <div class="settings-field">
          <label>Max suggestions</label>
          <div><input type="number" id="at-max" data-key="auto_tag.max_tags" min="1" max="12" value="${Number(t.max_tags) || 5}" style="width:80px;" /></div>
        </div>

        <div class="settings-field">
          <label>Instructions</label>
          <div>
            <textarea id="at-prompt" data-key="auto_tag.prompt" rows="3" style="width:100%; resize:vertical; font-family:inherit;"
              placeholder="How the AI should pick tags (your tag list and the transcript are appended automatically)">${escapeHtml(t.prompt ?? "")}</textarea>
          </div>
        </div>
      </div>
    `;

    this.container.querySelector<HTMLInputElement>("#at-auto")?.addEventListener("change", (e) => {
      t.auto = (e.target as HTMLInputElement).checked;
    });
    this.container.querySelector<HTMLSelectElement>("#at-provider")?.addEventListener("change", (e) => {
      t.provider = (e.target as HTMLSelectElement).value;
      this.render();
    });
    this.container.querySelector<HTMLInputElement>("#at-key")?.addEventListener("input", (e) => {
      t.api_key = (e.target as HTMLInputElement).value;
    });
    this.container.querySelector<HTMLInputElement>("#at-url")?.addEventListener("input", (e) => {
      t.api_url = (e.target as HTMLInputElement).value;
    });
    this.container.querySelector<HTMLInputElement>("#at-model")?.addEventListener("input", (e) => {
      t.model = (e.target as HTMLInputElement).value;
    });
    this.container.querySelector<HTMLInputElement>("#at-max")?.addEventListener("input", (e) => {
      const n = Number((e.target as HTMLInputElement).value);
      t.max_tags = Number.isFinite(n) ? Math.max(1, Math.min(12, Math.round(n))) : 5;
    });
    this.container.querySelector<HTMLTextAreaElement>("#at-prompt")?.addEventListener("input", (e) => {
      t.prompt = (e.target as HTMLTextAreaElement).value;
    });
  }
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(s: string): string {
  return escapeHtml(s).replace(/"/g, "&quot;");
}
