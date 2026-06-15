import { renderField, bindFieldEvents } from "./form";
import { mountModelField } from "./modelField";
import { mountConnectionField, deriveConnectionEntry } from "./connectionField";

/**
 * Settings → Post-Processing: every LLM-over-the-transcript feature in one
 * tab — cleanup (`config.llm_post_process`: enable, the shared
 * connection/model fields, prompt, timeout, Ollama autostart), auto-summary
 * (`config.summary`), and auto titles (`config.title`, heuristic vs LLM).
 * The summary/title connection fields INHERIT the cleanup connection when
 * left blank — mirroring the daemon's fallback — so most users configure one
 * provider here and everything else rides it.
 *
 * The constructor seeds any missing config tables with the documented
 * defaults before rendering (older configs predate these features). Plain
 * section class composing the shared connectionField/modelField mounts over
 * the form.ts binding.
 */
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
        timeout_secs: 30,
        autostart_ollama: true
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

    // Auto titles. The heuristic (first meaningful sentence) is free and on
    // by default; the LLM pass is opt-in and falls back to the heuristic on
    // any error. Connection fields inherit the cleanup connection when blank,
    // exactly like summaries. The prompt default mirrors the daemon's.
    if (!config.title) {
      config.title = {
        enabled: true,
        use_llm: false,
        provider: "",
        api_key: "",
        api_url: "",
        model: "",
        prompt: "You title voice-note transcripts. Reply with ONLY a short title for the transcript: at most 8 words, plain text, no quotes, no trailing punctuation, no preamble.",
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
        <h3>AI Post-Processing</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Automatically edit, reformat, and clean up your transcript using a local or remote LLM.
        </p>

        <div style="background-color: var(--bg-deep); padding: 12px; border-radius: 6px; border: 1px solid var(--border-subtle); margin-bottom: 16px;">
          <strong style="display: block; font-size: 0.9286rem; margin-bottom: 6px; color: var(--fg-default);">How to use this for free (Offline):</strong>
          <ol style="margin: 0; padding-left: 20px; font-size: 0.8571rem; color: var(--fg-muted); line-height: 1.5;">
            <li>Download and install <a href="#" id="ollama-download-link" style="color: var(--accent); text-decoration: none;">Ollama</a>.</li>
            <li>Open your terminal and run <code>ollama run llama3.2:3b</code>.</li>
            <li>Select <strong>Ollama</strong> below and use <code>llama3.2:3b</code> as your Model Name!</li>
          </ol>
        </div>

        <div class="settings-field">
          <label>Enable AI Post-Processing</label>
          <div>${renderField(
            { key: "llm_post_process.enabled", label: "", kind: "checkbox" },
            lp.enabled,
          )}</div>
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
              lp.prompt || "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.",
            )}
            <span class="settings-help-text">
              Tell the AI how to edit your transcript. Finish with "Reply ONLY with the final text."
            </span>
          </div>
        </div>

        <hr style="border: none; border-top: 1px solid var(--border-subtle); margin: 20px 0 16px;" />

        <h3 style="margin-bottom: 4px;">Auto AI Summary</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
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
          <span style="font-size: 0.7857rem; color: var(--fg-faded); grid-column: 2;">
            When off, summaries are still available on demand per recording.
          </span>
        </div>

        <div class="settings-field conn-field">
          <label>Summary provider</label>
          <div id="summary-conn-host"></div>
        </div>

        <div class="settings-field">
          <label>Summary model (optional)</label>
          <div id="summary-model-host"></div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); grid-column: 2;">
            Leave on "Same as cleanup model" to reuse the post-processing model, or pick a different
            one (e.g. a smaller/faster model just for summaries).
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

        <hr style="border: none; border-top: 1px solid var(--border-subtle); margin: 20px 0 16px;" />

        <h3 style="margin-bottom: 4px;">Auto Titles</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Name each recording from its first meaningful sentence — free, instant, and fully
          offline. Optionally let the AI write a short title instead; if the AI fails for any
          reason, the built-in heuristic still applies. A title you type on a recording yourself
          is <b>never</b> overwritten; clear it (save an empty title) to go back to automatic.
        </p>

        <div class="settings-field">
          <label>Title every recording</label>
          <div>${renderField(
            { key: "title.enabled", label: "", kind: "checkbox" },
            config.title.enabled,
          )}</div>
        </div>

        <div class="settings-field">
          <label>Use the AI for titles</label>
          <div>${renderField(
            { key: "title.use_llm", label: "", kind: "checkbox" },
            config.title.use_llm,
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); grid-column: 2;">
            Off = the built-in heuristic (first sentence of the transcript). On = ask the model
            below for a short title, falling back to the heuristic on any error.
          </span>
        </div>

        <div id="title-llm-fields" style="${config.title.use_llm ? "" : "display: none;"}">
          <div class="settings-field conn-field">
            <label>Title provider</label>
            <div id="title-conn-host"></div>
          </div>

          <div class="settings-field">
            <label>Title model (optional)</label>
            <div id="title-model-host"></div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); grid-column: 2;">
              Leave on "Same as cleanup model" to reuse the post-processing model, or pick a
              small/fast one just for titles.
            </span>
          </div>

          <div class="settings-field">
            <label>Title instructions</label>
            <div>
              <textarea data-key="title.prompt" rows="3" style="width:100%; resize:vertical; font-family:inherit;"
                placeholder="How the AI should title the transcript (the transcript is appended automatically)">${escapeHtml(config.title.prompt ?? "")}</textarea>
            </div>
          </div>
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
    // keystroke in the prompt or timeout must not reset the picker.
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

    // ── Summary connection + model ───────────────────────────────────────────
    // Same connection block with a leading "Same as Post-Processing" anchor:
    // choosing it blanks the summary's own provider/url/key, which is the
    // daemon's inherit-when-blank contract. The model field lists models for
    // the EFFECTIVE connection (the summary's own fields, falling back
    // per-field to the cleanup connection — what will actually run).
    const summaryEff = (which: "provider" | "api_url" | "api_key") => {
      const s = (config.summary[which] ?? "").toString().trim();
      return s || (lp[which] ?? "").toString();
    };
    const summaryModelHost = container.querySelector<HTMLElement>("#summary-model-host");
    let summaryMountKey: string | null = null;
    const mountSummaryModel = () => {
      if (!summaryModelHost) return;
      const key = `${summaryEff("provider")}|${summaryEff("api_url")}|${summaryEff("api_key")}`;
      if (key === summaryMountKey) return;
      summaryMountKey = key;
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

    const summaryConnHost = container.querySelector<HTMLElement>("#summary-conn-host");
    if (summaryConnHost) {
      mountConnectionField(summaryConnHost, {
        catalog: "llm",
        inheritLabel: "Same as Post-Processing",
        getKind: () => (config.summary.provider ?? "").toString(),
        setKind: (k) => { config.summary.provider = k; },
        getApiUrl: () => (config.summary.api_url ?? "").toString(),
        setApiUrl: (u) => { config.summary.api_url = u; },
        getApiKey: () => (config.summary.api_key ?? "").toString(),
        setApiKey: (k) => { config.summary.api_key = k; },
        onProviderChanged: () => mountSummaryModel(),
      });
      summaryConnHost.addEventListener("input", () => mountSummaryModel());
    }
    mountSummaryModel();

    // The cleanup connection is also the summary's fallback: while the summary
    // inherits (blank fields), a cleanup provider/url/key edit changes what the
    // summary model field should list. "input" covers typed url/key edits,
    // "change" the provider select; the mount-key guard drops the no-ops.
    cleanupConnHost?.addEventListener("input", () => mountSummaryModel());
    cleanupConnHost?.addEventListener("change", () => mountSummaryModel());

    // ── Title connection + model ─────────────────────────────────────────────
    // Same shared blocks as the summary: a leading "Same as Post-Processing"
    // anchor blanks the title's own provider/url/key (the daemon's
    // inherit-when-blank contract), and the model field lists models for the
    // EFFECTIVE connection (own fields falling back per-field to cleanup).
    const t = config.title;
    const titleEff = (which: "provider" | "api_url" | "api_key") => {
      const own = (t[which] ?? "").toString().trim();
      return own || (lp[which] ?? "").toString();
    };
    const titleModelHost = container.querySelector<HTMLElement>("#title-model-host");
    let titleMountKey: string | null = null;
    const mountTitleModel = () => {
      if (!titleModelHost) return;
      const key = `${titleEff("provider")}|${titleEff("api_url")}|${titleEff("api_key")}`;
      if (key === titleMountKey) return;
      titleMountKey = key;
      mountModelField(titleModelHost, {
        mode: "llm",
        blankLabel: "Same as cleanup model",
        getProvider: () => titleEff("provider"),
        getApiUrl: () => titleEff("api_url"),
        getApiKey: () => titleEff("api_key"),
        getModel: () => t.model || "",
        setModel: (m) => { t.model = m; },
      });
    };

    const titleConnHost = container.querySelector<HTMLElement>("#title-conn-host");
    if (titleConnHost) {
      mountConnectionField(titleConnHost, {
        catalog: "llm",
        inheritLabel: "Same as Post-Processing",
        getKind: () => (t.provider ?? "").toString(),
        setKind: (k) => { t.provider = k; },
        getApiUrl: () => (t.api_url ?? "").toString(),
        setApiUrl: (u) => { t.api_url = u; },
        getApiKey: () => (t.api_key ?? "").toString(),
        setApiKey: (k) => { t.api_key = k; },
        onProviderChanged: () => mountTitleModel(),
      });
      titleConnHost.addEventListener("input", () => mountTitleModel());
    }
    mountTitleModel();

    // While the title inherits, cleanup connection edits change what its model
    // field should list — same forwarding the summary block does.
    cleanupConnHost?.addEventListener("input", () => mountTitleModel());
    cleanupConnHost?.addEventListener("change", () => mountTitleModel());

    // The provider/model/prompt rows only matter when the LLM pass is on; the
    // heuristic needs no configuration.
    const titleLlmFields = container.querySelector<HTMLElement>("#title-llm-fields");
    container
      .querySelector<HTMLInputElement>("[data-key='title.use_llm']")
      ?.addEventListener("change", (e) => {
        if (titleLlmFields) {
          titleLlmFields.style.display = (e.target as HTMLInputElement).checked ? "" : "none";
        }
      });
  }
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
