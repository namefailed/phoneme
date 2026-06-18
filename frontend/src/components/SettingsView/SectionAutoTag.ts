/**
 * Auto-tagging settings (Post-Processing tab). The LLM proposes tags for each
 * transcript — preferring tags you already use — and the proposals wait as
 * dashed chips in the recording's tag row until you approve or dismiss them.
 * Exception: with "auto-accept existing tags" on, a suggestion that matches a
 * tag you ALREADY use is attached immediately — only brand-new names wait for
 * approval.
 *
 * The tag PROMPT, provider and model now live in the Playbook's "Auto-tag"
 * entry (the daemon resolves the tag step — automatic and on-demand — from that
 * entry), so this section keeps only the tag-specific BEHAVIOUR that isn't part
 * of an LLM entry: whether tagging runs automatically, whether existing-tag
 * matches auto-apply, how many suggestions to ask for, and the library-wide
 * "clear pending suggestions" sweep. A link jumps to the Playbook entry for the
 * wording/model.
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
        <p style="font-size: 0.8571rem; color:var(--fg-muted); margin:0 0 12px; line-height:1.45;">
          Let the AI propose tags for each new transcript (it prefers tags you already use).
          Proposals appear as dashed ✨ chips on the recording — <b>you approve or dismiss
          each one</b>. The ✨ Suggest button on a recording runs this on demand even when the
          automatic step is off.
        </p>

        <div style="background-color: var(--bg-deep); padding: 10px 12px; border-radius: 6px; border: 1px solid var(--border-subtle); margin-bottom: 16px; font-size: 0.8571rem; color: var(--fg-muted); line-height: 1.5;">
          🎭 <strong style="color: var(--fg-default);">Wording &amp; model live in the Playbook.</strong>
          The prompt, provider and model for tag suggestions are the Playbook's
          <strong>Auto-tag</strong> entry — edit them there and both the automatic step and the
          ✨ Suggest button follow.
          <a href="#" id="at-open-playbook" style="color: var(--accent); text-decoration: none; white-space: nowrap;">Open the Playbook →</a>
        </div>

        <div class="settings-field">
          <label>Suggest tags automatically</label>
          <div><input type="checkbox" class="toggle-switch" id="at-auto" data-key="auto_tag.auto" ${t.auto ? "checked" : ""} /></div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); grid-column: 2;">
            Run tag suggestions on every new recording. Off leaves tagging to the ✨ Suggest
            button (and to any Custom Hotkey recipe that includes the Auto-tag step).
          </span>
        </div>

        <div class="settings-field">
          <label>Auto-apply existing tags
            <br><span style="font-size: 0.7857rem; color:var(--fg-muted); font-weight:normal;">A suggestion matching a tag you already have (e.g. <code>code</code>) is applied immediately; only brand-new tag names wait for approval.</span>
          </label>
          <div><input type="checkbox" class="toggle-switch" id="at-accept" data-key="auto_tag.auto_accept_existing" ${t.auto_accept_existing ? "checked" : ""} /></div>
        </div>

        <div class="settings-field">
          <label>Max tag suggestions</label>
          <div><input type="number" id="at-max" data-key="auto_tag.max_tags" min="1" max="12" value="${Number(t.max_tags) || 5}" style="width:80px;" /></div>
        </div>

        <div class="settings-field">
          <label>Pending tag suggestions</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <button class="inline-button" id="at-clear-all" title="Remove every pending ✨ suggestion chip from every recording in the library">🧹 Clear all suggestions</button>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              Removes every pending suggestion chip across the whole library in one sweep.
              Tags that were already approved stay attached — this only discards the
              not-yet-decided proposals.
            </span>
          </div>
        </div>
      </div>
    `;

    // Jump to the Playbook tab (its Auto-tag entry) where the prompt/model are
    // authored — the same in-app navigation Post-Processing uses, so the
    // unsaved-edits guard stays in play.
    this.container
      .querySelector<HTMLAnchorElement>("#at-open-playbook")
      ?.addEventListener("click", (e) => {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("phoneme:navigate", {
          detail: { view: "settings", section: "managers/playbook" },
        }));
      });

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
  }
}
