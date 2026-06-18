import { renderField, bindFieldEvents } from "./form";
import { mountModelField } from "./modelField";
import { mountConnectionField, deriveConnectionEntry } from "./connectionField";

/**
 * Settings → Post-Processing: the BASE AI connection for every LLM-over-the-
 * transcript step. The individual instructions (cleanup, summary, title, tags)
 * now live in the Playbook — each Playbook entry carries its own
 * provider/model/prompt and INHERITS this connection wherever its own fields are
 * left blank. So this section is deliberately thin: the master on/off switch,
 * the shared provider/endpoint/key/model + timeout + Ollama autostart, and a
 * link over to the Playbook where the actual instructions are authored.
 *
 * `llm_post_process.enabled` is still the master switch the migration reads to
 * decide whether the `default` recipe includes the `cleanup` step, so its key /
 * semantics are unchanged — only the label/help are reframed.
 *
 * The constructor seeds `config.llm_post_process` with the documented defaults
 * before rendering (older configs predate this feature). Plain section class
 * composing the shared connectionField/modelField mounts over the form.ts
 * binding.
 */
export class SectionPostProcessing {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    // Ensure the nested object exists in the loaded config to prevent undefined
    // errors. `prompt` is preserved for round-trip / the migration source, but
    // it is no longer authored here — the Playbook's `cleanup` entry owns it.
    if (!config.llm_post_process) {
      config.llm_post_process = {
        enabled: false,
        provider: "none",
        api_key: "",
        api_url: "",
        model: "llama3.2:3b",
        prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.",
        timeout_secs: 30,
        autostart_ollama: true
      };
    }

    const lp = config.llm_post_process;

    // The effective cleanup connection. The shared connection block writes the
    // config live (no per-provider duplicate inputs anymore), so the config IS
    // the current form state.
    const cleanupEff = (which: "provider" | "api_url" | "api_key"): string =>
      (lp[which] ?? (which === "provider" ? "none" : "")).toString();

    container.innerHTML = `
      <div class="settings-section">
        <h3>AI Connection</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          The shared AI provider for editing, summarizing, titling, and tagging your transcripts.
          This is the <strong>base connection</strong> every Playbook step inherits when its own
          provider/model fields are left blank.
        </p>

        <div style="background-color: var(--bg-deep); padding: 10px 12px; border-radius: 6px; border: 1px solid var(--border-subtle); margin-bottom: 16px; font-size: 0.8571rem; color: var(--fg-muted); line-height: 1.5;">
          🎭 <strong style="color: var(--fg-default);">Instructions live in the Playbook.</strong>
          Cleanup, summary, title, and tag prompts (and their per-step model overrides) are now
          authored in the <strong>Playbook</strong>. This section just sets the connection they
          fall back to.
          <a href="#" id="open-playbook-link" style="color: var(--accent); text-decoration: none; white-space: nowrap;">Open the Playbook →</a>
        </div>

        <div style="background-color: var(--bg-deep); padding: 10px 12px; border-radius: 6px; border: 1px solid var(--border-subtle); margin-bottom: 16px; font-size: 0.8571rem; color: var(--fg-muted); line-height: 1.5;">
          💡 <strong style="color: var(--fg-default);">Free &amp; offline:</strong> the first-run setup wizard installs
          <a href="#" id="ollama-download-link" style="color: var(--accent); text-decoration: none;">Ollama</a> and pulls a model for
          you — just pick <strong>Ollama</strong> as the provider below. Setting it up yourself? Install Ollama, then select it here.
        </div>

        <div class="settings-field">
          <label>Enable AI Post-Processing</label>
          <div>${renderField(
            { key: "llm_post_process.enabled", label: "", kind: "checkbox" },
            lp.enabled,
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); grid-column: 2;">
            Master switch for AI steps. When off, the default recording pipeline skips its
            cleanup step. Individual steps are still chosen in the Playbook.
          </span>
        </div>

        <div class="settings-field conn-field">
          <label>AI Provider</label>
          <div id="cleanup-conn-host"></div>
        </div>

        <div class="settings-field" id="cleanup-model-field" style="display: none;">
          <label>Model</label>
          <div id="cleanup-model-host"></div>
        </div>

        <div id="llm-cloud-note" style="display:none; border:1px solid var(--err); border-radius:6px; padding:8px 10px; margin:4px 0 12px; font-size: 0.8571rem; line-height:1.45;">
          ⚠️ <b>Cloud post-processing.</b> Your transcript text is sent to this provider's servers for processing. Use <b>Ollama</b> to keep everything offline.
        </div>

        <div class="settings-field">
          <label>Start Ollama automatically</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "llm_post_process.autostart_ollama", label: "", kind: "checkbox" },
              lp.autostart_ollama ?? true,
            )}</div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              When an AI step (cleanup, summary, tags, titles) points at a local Ollama that isn't
              running, launch <code>ollama serve</code> on demand and stop it again when the engine
              shuts down. An Ollama you started yourself is detected and never touched. Only ever
              applies to local Ollama connections.
            </span>
          </div>
        </div>

        <div class="settings-field" id="llm-timeout-field" style="display:none;">
          <label>Request timeout (seconds)</label>
          <div>${renderField(
            { key: "llm_post_process.timeout_secs", label: "", kind: "number" },
            lp.timeout_secs ?? 30,
          )}</div>
        </div>
      </div>
    `;

    bindFieldEvents(container, config);

    // ── Cleanup model field ──────────────────────────────────────────────────
    // One shared model picker (curated suggestions + live fetch + "Other…"), fed
    // the EFFECTIVE cleanup connection so it always lists models for whatever
    // provider/url/key is currently set. The field owns its host's DOM and
    // writes config.llm_post_process.model itself.
    const cleanupModelHost = container.querySelector<HTMLElement>("#cleanup-model-host");
    // Re-mount only when the connection actually changes (provider|url|key); a
    // keystroke in the timeout must not reset the picker.
    let cleanupMountKey = "";
    const mountCleanupModel = () => {
      if (!cleanupModelHost) return;
      const key = `${cleanupEff("provider")}|${cleanupEff("api_url")}|${cleanupEff("api_key")}`;
      if (key === cleanupMountKey) return;
      cleanupMountKey = key;
      mountModelField(cleanupModelHost, {
        mode: "llm",
        getProvider: () => cleanupEff("provider"),
        getApiUrl: () => cleanupEff("api_url"),
        getApiKey: () => cleanupEff("api_key"),
        getModel: () => lp.model || "",
        setModel: (m) => { lp.model = m; },
      });
    };

    // Model + timeout apply to every active provider; the privacy note only to
    // connections that actually leave the machine (cloud providers and custom
    // endpoints — local servers like LM Studio share the openai wire kind, so
    // the GROUP from the derived entry is what tells them apart, not the kind).
    const updateCleanupVisibility = () => {
      const provider = cleanupEff("provider");
      const off = provider === "none" || provider === "";
      const group = deriveConnectionEntry("llm", provider, cleanupEff("api_url"))?.group;
      const modelEl = container.querySelector<HTMLElement>("#cleanup-model-field");
      if (modelEl) modelEl.style.display = off ? "none" : "grid";
      const timeoutEl = container.querySelector<HTMLElement>("#llm-timeout-field");
      if (timeoutEl) timeoutEl.style.display = off ? "none" : "";
      const cloudNote = container.querySelector<HTMLElement>("#llm-cloud-note");
      if (cloudNote) cloudNote.style.display = !off && (group === "cloud" || group === "advanced") ? "" : "none";
    };

    // ── Cleanup connection ───────────────────────────────────────────────────
    // The shared connection block: grouped named providers, key row when
    // needed, Test, endpoint under Advanced. Writes the same config keys the
    // old provider dropdown + per-provider url/key rows did.
    const cleanupConnHost = container.querySelector<HTMLElement>("#cleanup-conn-host");
    if (cleanupConnHost) {
      mountConnectionField(cleanupConnHost, {
        catalog: "llm",
        getKind: () => cleanupEff("provider"),
        setKind: (k) => { lp.provider = k; },
        getApiUrl: () => cleanupEff("api_url"),
        setApiUrl: (u) => { lp.api_url = u; },
        getApiKey: () => cleanupEff("api_key"),
        setApiKey: (k) => { lp.api_key = k; },
        onProviderChanged: () => {
          updateCleanupVisibility();
          mountCleanupModel();
        },
      });
      // Key/url edits inside the block don't fire onProviderChanged (that's
      // for provider switches) but the model list must follow the credentials;
      // the mount-key guard absorbs the keystroke churn into real re-mounts.
      cleanupConnHost.addEventListener("input", () => mountCleanupModel());
    }
    updateCleanupVisibility();
    mountCleanupModel();

    // Jump to the Playbook tab where the cleanup/summary/title/tag instructions
    // are authored. Uses the same in-app navigation the rest of Settings uses
    // (the `phoneme:navigate` window event → App.tryNavigate → the Playbook
    // tab); routing it through App keeps the unsaved-edits guard in play.
    container
      .querySelector<HTMLAnchorElement>("#open-playbook-link")
      ?.addEventListener("click", (e) => {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("phoneme:navigate", {
          detail: { view: "settings", section: "managers/playbook" },
        }));
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
  }
}
