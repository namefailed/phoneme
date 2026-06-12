import { mountModelField } from "./modelField";
import { mountConnectionField } from "./connectionField";

/**
 * Auto-tagging settings (Post-Processing tab). The LLM proposes tags for each
 * transcript — preferring tags you already use — and the proposals wait as
 * dashed chips in the recording's tag row until you approve or dismiss them.
 * Exception: with "auto-accept existing tags" on, a suggestion that matches a
 * tag you ALREADY use is attached immediately — only brand-new names wait for
 * approval.
 *
 * Connection fields mirror the Summary section: blank provider/key/URL/model
 * inherit the `[llm_post_process]` (cleanup) connection, so the common case is
 * just flipping the toggle on. The provider/key/endpoint UI is the shared
 * connection block with a leading "Same as Post-Processing" anchor.
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
        auto_accept_existing: false,
      };
    }
    this.render();
  }

  private render() {
    const t = this.config.auto_tag;

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Auto-Tagging</h3>
        <p style="font-size:12px; color:var(--fg-muted); margin:0 0 4px;">
          Let the AI propose tags for each new transcript (it prefers tags you already use).
          Proposals appear as dashed ✨ chips on the recording — <b>you approve or dismiss
          each one</b>. With "auto-accept existing tags" on, matches of tags you already
          use attach immediately; only brand-new names wait for approval. The ✨ Suggest button on a recording
          runs this on demand even when the automatic step is off.
        </p>

        <div class="settings-field">
          <label>Suggest tags automatically</label>
          <div><input type="checkbox" class="toggle-switch" id="at-auto" data-key="auto_tag.auto" ${t.auto ? "checked" : ""} /></div>
        </div>

        <div class="settings-field">
          <label>Auto-apply existing tags
            <br><span style="font-size:11px; color:var(--fg-muted); font-weight:normal;">A suggestion matching a tag you already have (e.g. <code>code</code>) is applied immediately; only brand-new tag names wait for approval.</span>
          </label>
          <div><input type="checkbox" class="toggle-switch" id="at-accept" data-key="auto_tag.auto_accept_existing" ${t.auto_accept_existing ? "checked" : ""} /></div>
        </div>

        <div class="settings-field stacked">
          <label>Provider</label>
          <div id="at-conn-host"></div>
        </div>

        <div class="settings-field">
          <label>Model</label>
          <div id="at-model-host"></div>
        </div>

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

        <div class="settings-field">
          <label>Pending suggestions</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <button class="inline-button" id="at-clear-all" title="Remove every pending ✨ suggestion chip from every recording in the library">🧹 Clear all suggestions</button>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Removes every pending suggestion chip across the whole library in one sweep.
              Tags that were already approved stay attached — this only discards the
              not-yet-decided proposals.
            </span>
          </div>
        </div>
      </div>
    `;

    // Model — the shared model field, fed the EFFECTIVE connection: auto-tag's
    // own provider/key/URL when set, else the inherited cleanup connection
    // (blank auto_tag fields fall back to [llm_post_process] in the daemon, so
    // the dropdown must list models for whatever will actually run). The field
    // owns the host's DOM and writes the model straight into the config.
    const eff = (which: "provider" | "api_url" | "api_key") => {
      const own = (t[which] ?? "").toString().trim();
      return own || (this.config.llm_post_process?.[which] ?? "").toString();
    };
    const modelHost = this.container.querySelector<HTMLElement>("#at-model-host");
    let modelMountKey: string | null = null;
    const mountAtModel = () => {
      if (!modelHost) return;
      const key = `${eff("provider")}|${eff("api_url")}|${eff("api_key")}`;
      if (key === modelMountKey) return;
      modelMountKey = key;
      mountModelField(modelHost, {
        mode: "llm",
        blankLabel: "Same as cleanup model",
        getProvider: () => eff("provider"),
        getApiUrl: () => eff("api_url"),
        getApiKey: () => eff("api_key"),
        getModel: () => t.model || "",
        setModel: (m) => { t.model = m; },
      });
    };

    // Provider/key/endpoint — the shared connection block. The leading "Same
    // as Post-Processing" option blanks provider/url/key (the daemon's
    // inherit-when-blank contract, same as the Summary section).
    const connHost = this.container.querySelector<HTMLElement>("#at-conn-host");
    if (connHost) {
      mountConnectionField(connHost, {
        catalog: "llm",
        inheritLabel: "Same as Post-Processing",
        getKind: () => (t.provider ?? "").toString(),
        setKind: (k) => { t.provider = k; },
        getApiUrl: () => (t.api_url ?? "").toString(),
        setApiUrl: (u) => { t.api_url = u; },
        getApiKey: () => (t.api_key ?? "").toString(),
        setApiKey: (k) => { t.api_key = k; },
        onProviderChanged: () => mountAtModel(),
      });
      // Key/url keystrokes don't fire onProviderChanged; the model list still
      // follows them (the mount-key guard drops the no-ops).
      connHost.addEventListener("input", () => mountAtModel());
    }
    mountAtModel();

    this.container.querySelector<HTMLButtonElement>("#at-clear-all")?.addEventListener("click", async () => {
      const { confirmDialog } = await import("../confirmDialog");
      const ok = await confirmDialog({
        title: "Clear all suggestions?",
        body: "Every pending tag suggestion on every recording will be discarded. Approved tags are not touched.",
        confirmLabel: "Clear all",
        danger: true,
      });
      if (!ok) return;
      try {
        const { clearAllTagSuggestions } = await import("../../services/ipc");
        const n = await clearAllTagSuggestions();
        const { showToast } = await import("../../utils/toast");
        showToast(
          n === 0
            ? "No pending suggestions to clear"
            : `Cleared suggestions on ${n} recording${n === 1 ? "" : "s"}`,
          "success",
        );
      } catch (e) {
        const { showToast } = await import("../../utils/toast");
        const { errText } = await import("../../utils/error");
        showToast(`Couldn't clear suggestions: ${errText(e)}`, "error");
      }
    });

    this.container.querySelector<HTMLInputElement>("#at-auto")?.addEventListener("change", (e) => {
      t.auto = (e.target as HTMLInputElement).checked;
    });
    this.container.querySelector<HTMLInputElement>("#at-accept")?.addEventListener("change", (e) => {
      t.auto_accept_existing = (e.target as HTMLInputElement).checked;
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
