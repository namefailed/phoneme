import { escapeHtml as escHtml, escapeAttr as escAttr } from "../../utils/format";
/**
 * The one connection block behind every provider picker: a grouped select of
 * named providers (the brand the user knows — "On this computer" / "Cloud" /
 * "Advanced"), an API key row that appears only when the provider needs one,
 * a Test button that proves the connection inline, and the endpoint URL tucked
 * under a collapsed Advanced disclosure. Used by the Post-Processing, Summary,
 * Auto-Tag, Transcription and Live Preview settings plus the Models modal.
 *
 * The block reads and writes the config shape directly — the wire `provider`
 * kind plus `api_url` — through the caller's getters/setters. Picking a named
 * provider writes that provider's kind and default endpoint; the current
 * selection is derived back from (kind, api_url) via the catalog matchers, so
 * saved configs round-trip with zero migration and a hand-edited TOML that
 * matches nothing simply displays as Custom.
 *
 * Pure vanilla DOM so it drops into innerHTML-based settings sections and the
 * Lit-rendered Models modal alike. Mounting onto the same host supersedes the
 * previous mount (the block owns host.innerHTML).
 */
import { fetchLlmModels, MASKED_SECRET } from "../../services/llmModels";
import { LLM_PRESETS } from "../../services/llmProviders";
import { STT_NAMED_PROVIDERS, matchNamedSttProvider } from "../../services/sttProviders";
import { errText } from "../../utils/error";

/** What a caller wires into {@link mountConnectionField}: which provider
 *  catalog to offer, and live getters/setters onto its config shape. */
export interface ConnectionFieldOpts {
  catalog: "llm" | "stt";
  // Each reads/writes the live config shape for the step:
  getKind(): string;
  setKind(k: string): void; // wire provider
  getApiUrl(): string;
  setApiUrl(u: string): void;
  getApiKey(): string;
  setApiKey(k: string): void;
  /** Leading inherit option, e.g. "Same as Post-Processing". When chosen,
   *  set kind/url/key to "" (the backend inherit contract). Omit = none. */
  inheritLabel?: string;
  /** Re-mounted model field host is owned by the caller; the connection
   *  block calls this after provider changes so the model list follows. */
  onProviderChanged?: () => void;
  /** Optional STT local test URL resolver for the Test button. */
  resolveTestUrl?: () => string;
}

/** A named provider as the connection block sees it, whichever catalog it
 *  came from (LLM presets or the named STT providers). */
export interface ConnectionEntry {
  id: string;
  label: string;
  group: "local" | "cloud" | "advanced";
  /** Wire kind written to the config on selection. */
  kind: string;
  /** `api_url` written on selection. Blank = the kind's built-in default. */
  defaultUrl: string;
  needsKey: boolean;
  keyUrl?: string;
  modelsListable: boolean;
  hint: string;
}

/** The LLM escape hatch. Not a preset in `LLM_PRESETS` (it has no endpoint of
 *  its own, so the quick-preset consumers — wizard, re-run form — never list
 *  it); selecting it keeps the current URL for the user to override under
 *  Advanced. The one place allowed to say "OpenAI-compatible". */
const LLM_CUSTOM_ENTRY: ConnectionEntry = {
  id: "custom",
  label: "Custom (OpenAI-compatible)",
  group: "advanced",
  kind: "openai",
  defaultUrl: "",
  needsKey: true,
  modelsListable: true,
  hint: "Any OpenAI-compatible chat endpoint — your own server or a gateway. Set the URL under Advanced.",
};

/** Sentinel option value for the leading inherit entry. */
const INHERIT = "__inherit__";
/** Wire value the cleanup step uses for "post-processing off". */
const NONE = "none";

const LLM_ENTRIES: ConnectionEntry[] = [
  ...LLM_PRESETS.map((p) => ({
    id: p.id,
    label: p.label,
    group: p.group,
    kind: p.kind,
    defaultUrl: p.apiUrl,
    needsKey: p.needsKey,
    keyUrl: p.keyUrl,
    modelsListable: p.modelsListable,
    hint: p.hint,
  })),
  LLM_CUSTOM_ENTRY,
];

/** Every named entry for a catalog, in display order. */
export function connectionEntries(catalog: "llm" | "stt"): ConnectionEntry[] {
  return catalog === "llm" ? LLM_ENTRIES : STT_NAMED_PROVIDERS;
}

/**
 * The named entry a stored (kind, api_url) displays as — the derivation that
 * makes configs round-trip. LLM: "none" means off, an exact URL match (slash
 * tolerant) names the provider, a blank URL falls back to the kind's canonical
 * entry, anything else is Custom. STT kinds map 1:1 onto named providers.
 */
export function deriveConnectionId(catalog: "llm" | "stt", kind: string, apiUrl: string): string {
  const k = (kind || "").trim();
  if (catalog === "llm") {
    if (k === NONE) return NONE; // off is off, whatever URL is left behind
    const url = (apiUrl || "").trim().replace(/\/+$/, "");
    if (url) {
      const byUrl = LLM_ENTRIES.find((e) => e.defaultUrl.replace(/\/+$/, "") === url);
      if (byUrl) return byUrl.id;
    } else {
      // Blank URL = the kind's built-in default endpoint → its canonical entry.
      const canonical = LLM_ENTRIES.find((e) => e.kind === k && e.id === k);
      if (canonical) return canonical.id;
    }
    return k ? "custom" : NONE;
  }
  // STT: kinds map 1:1; unknown kinds display as the Custom escape hatch.
  // A blank kind (no own connection mounted without an inherit option — a
  // transient state at worst) shows the daemon default.
  return matchNamedSttProvider(k, apiUrl)?.id ?? "local";
}

/** `deriveConnectionId`, resolved to the entry (undefined for none/inherit). */
export function deriveConnectionEntry(
  catalog: "llm" | "stt",
  kind: string,
  apiUrl: string,
): ConnectionEntry | undefined {
  const id = deriveConnectionId(catalog, kind, apiUrl);
  return connectionEntries(catalog).find((e) => e.id === id);
}


/** "Ollama (local)" → "Ollama", for friendly error copy. */
const plainName = (label: string) => label.replace(/\s*\((local|fast|cloud)\)\s*$/i, "").trim();

/** host:port of a URL, for "is it running?" errors; the raw string if unparsable. */
function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/**
 * Re-mounting onto the same host supersedes the previous mount: the token lets
 * a superseded mount's in-flight Test notice it lost ownership and drop its
 * late result instead of writing into the new block.
 */
const mountTokens = new WeakMap<HTMLElement, object>();

/** Render the connection block into `host` (owning host.innerHTML) and keep
 *  it live: selections/edits write straight through the opts setters, and
 *  provider changes re-render + call `onProviderChanged`. See the file-top
 *  comment for the full contract. */
export function mountConnectionField(host: HTMLElement, opts: ConnectionFieldOpts): void {
  const token = {};
  mountTokens.set(host, token);

  const entries = connectionEntries(opts.catalog);
  const byId = new Map(entries.map((e) => [e.id, e]));

  // Picking the LLM Custom escape hatch keeps the wire kind "openai", so a
  // config whose URL still matches a named provider would derive right back to
  // that provider. Remember the explicit choice for this mount so the select
  // doesn't snap away while the user opens Advanced to type their endpoint.
  let choseCustom = false;
  // The Advanced disclosure survives re-renders (e.g. a provider switch).
  let advancedOpen = false;
  // Inline Test outcome: cls "" = neutral note, "ok"/"err" = the usual chrome.
  let test: { cls: "" | "ok" | "err"; text: string } | null = null;
  let testing = false;

  const allBlank = () => !opts.getKind().trim() && !opts.getApiUrl().trim() && !opts.getApiKey();

  /** The option the select should show right now. */
  const selectedId = (): string => {
    if (opts.inheritLabel && allBlank()) return INHERIT;
    if (choseCustom) return "custom";
    return deriveConnectionId(opts.catalog, opts.getKind(), opts.getApiUrl());
  };

  /** Whether the Test button exists for this entry, and which probe it runs. */
  const testKind = (entry: ConnectionEntry): "models" | "whisper" | "note" | "none" => {
    if (entry.modelsListable) return "models";
    if (opts.catalog === "stt" && (entry.kind === "local" || entry.kind === "custom")) {
      // Local probes the running server (the caller resolves its URL); custom
      // probes the user's endpoint. No resolver and no URL → nothing to probe.
      if (entry.kind === "local" && !opts.resolveTestUrl) return "none";
      return "whisper";
    }
    // No cheap probe (Deepgram/AssemblyAI/ElevenLabs, Perplexity): say so
    // instead of showing a button that could only fail.
    return entry.needsKey ? "note" : "none";
  };

  const render = () => {
    if (mountTokens.get(host) !== token) return; // superseded by a newer mount

    const sel = selectedId();
    const entry = byId.get(sel);
    const isInherit = sel === INHERIT;
    const isNone = sel === NONE;

    const opt = (v: string, label: string, selected: boolean) =>
      `<option value="${escAttr(v)}" ${selected ? "selected" : ""}>${escHtml(label)}</option>`;
    const groupHtml = (group: ConnectionEntry["group"], label: string) => {
      const list = entries.filter((e) => e.group === group);
      if (!list.length) return "";
      return `<optgroup label="${escAttr(label)}">${list.map((e) => opt(e.id, e.label, e.id === sel)).join("")}</optgroup>`;
    };
    const options = [
      opts.inheritLabel ? opt(INHERIT, opts.inheritLabel, isInherit) : "",
      // The cleanup step (and the modal's cleanup slot) can be switched off
      // entirely; steps with an inherit anchor blank instead of disabling.
      !opts.inheritLabel && opts.catalog === "llm" ? opt(NONE, "None", isNone) : "",
      groupHtml("local", "On this computer"),
      groupHtml("cloud", "Cloud"),
      groupHtml("advanced", "Advanced"),
    ].join("");

    const hint = isInherit
      ? "Nothing else to configure — this step reuses that connection."
      : isNone
        ? "Off — no AI model runs for this step."
        : (entry?.hint ?? "");

    const showRows = !isInherit && !isNone && !!entry;
    const probe = entry ? testKind(entry) : "none";
    // The URL override is pointless for the bundled local whisper server (it
    // doesn't read api_url; its own section manages the server connection).
    const showAdvanced = showRows && !(opts.catalog === "stt" && entry!.kind === "local");

    const keyRow =
      showRows && entry!.needsKey
        ? `<div class="cf-row">
             <label class="cf-mini-label">API key</label>
             <input type="password" class="cf-key" aria-label="API key" value="${escAttr(opts.getApiKey())}" autocomplete="off" spellcheck="false" />
             ${entry!.keyUrl ? `<a class="cf-key-link" href="${escAttr(entry!.keyUrl)}" target="_blank" rel="noreferrer">Get a key ↗</a>` : ""}
           </div>`
        : "";

    const testRow = !showRows
      ? ""
      : probe === "note"
        ? `<div class="cf-hint cf-test-note">No quick test for this provider — your key is used on the next ${
            opts.catalog === "stt" ? "transcription" : "run"
          }.</div>`
        : probe === "none"
          ? ""
          : `<div class="cf-row">
               <button type="button" class="inline-button cf-test" ${testing ? "disabled" : ""}>Test</button>
               ${
                 test
                   ? `<span class="cf-test-result ${test.cls === "ok" ? "test-result ok" : test.cls === "err" ? "test-result err" : ""}">${escHtml(test.text)}</span>`
                   : testing
                     ? `<span class="cf-test-result">Testing…</span>`
                     : ""
               }
             </div>`;

    const advanced = !showAdvanced
      ? ""
      : `<details class="settings-advanced cf-advanced" ${advancedOpen ? "open" : ""}>
           <summary>
             <svg class="settings-advanced-chev" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true">
               <path d="M9 6l6 6-6 6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
             </svg>
             Advanced
           </summary>
           <div class="cf-row" style="margin-top:8px;">
             <label class="cf-mini-label">Endpoint URL</label>
             <input type="text" class="cf-url" aria-label="Endpoint URL" value="${escAttr(opts.getApiUrl())}" placeholder="${escAttr(entry!.defaultUrl || "Provider default")}" spellcheck="false" />
           </div>
         </details>`;

    host.innerHTML = `
      <div class="cf">
        <div class="cf-row"><select class="cf-provider" aria-label="Provider">${options}</select></div>
        ${hint ? `<div class="cf-hint cf-provider-hint">${escHtml(hint)}</div>` : ""}
        ${keyRow}
        ${testRow}
        ${advanced}
      </div>`;

    bind();
  };

  /** A connection edit invalidates the last Test outcome. */
  const clearTest = () => {
    if (!test && !testing) return;
    test = null;
    const el = host.querySelector<HTMLElement>(".cf-test-result");
    if (el) el.remove();
  };

  const runTest = async () => {
    const entry = byId.get(selectedId());
    if (!entry || testing) return;
    testing = true;
    test = null;
    render();
    try {
      if (testKind(entry) === "models") {
        const key = opts.getApiKey();
        if (entry.needsKey && key === MASKED_SECRET) {
          // The daemon masks saved keys before they reach the WebView, so
          // there's nothing real to send. Don't pretend a probe happened.
          test = { cls: "", text: "Saved keys stay hidden — re-enter the key to test it." };
          return;
        }
        // STT clouds with a models endpoint (OpenAI/Groq) share the chat API's
        // host; their api_url points at the transcription route, so probe the
        // provider default instead of deriving /models from an audio path.
        const url = opts.catalog === "llm" ? opts.getApiUrl() : "";
        const models = await fetchLlmModels(entry.kind, url, key);
        test = { cls: "ok", text: `Connected — ${models.length} model${models.length === 1 ? "" : "s"}` };
      } else {
        const url = (opts.resolveTestUrl?.() ?? "").trim() || opts.getApiUrl().trim();
        if (!url) {
          // Local has no Advanced row (its own section manages the server),
          // so don't point at one that isn't there.
          test = {
            cls: "err",
            text:
              entry.kind === "local"
                ? "Nothing to test yet — no local server is configured."
                : "Set the endpoint URL under Advanced first.",
          };
          return;
        }
        const { invoke } = await import("@tauri-apps/api/core");
        const res = await invoke<{ ok: boolean; message: string }>("wizard_test_whisper", { url });
        test = { cls: res.ok ? "ok" : "err", text: res.message };
      }
    } catch (e) {
      const raw = errText(e);
      // A local server that isn't up fails with transport noise; say the
      // useful thing instead. Cloud errors pass through verbatim.
      test =
        entry.group === "local" && /fetch|network|connect/i.test(raw)
          ? {
              cls: "err",
              text: `Couldn't reach ${plainName(entry.label)} at ${hostOf(opts.getApiUrl() || entry.defaultUrl)} — is it running?`,
            }
          : { cls: "err", text: raw };
    } finally {
      testing = false;
      if (mountTokens.get(host) === token) render();
    }
  };

  const bind = () => {
    const select = host.querySelector<HTMLSelectElement>(".cf-provider")!;
    select.addEventListener("change", () => {
      const v = select.value;
      choseCustom = false;
      test = null;
      if (v === INHERIT) {
        // The backend inherit contract: blank fields fall back to the parent
        // step's connection.
        opts.setKind("");
        opts.setApiUrl("");
        opts.setApiKey("");
      } else if (v === NONE) {
        opts.setKind(NONE);
      } else {
        const e = byId.get(v);
        if (!e) return;
        if (e.group === "advanced") {
          // Escape hatch: write the wire kind but keep the current URL — the
          // user is about to type their own under Advanced.
          choseCustom = opts.catalog === "llm"; // stt custom derives back by kind
          advancedOpen = true;
          opts.setKind(e.kind);
        } else {
          opts.setKind(e.kind);
          opts.setApiUrl(e.defaultUrl);
        }
      }
      render();
      opts.onProviderChanged?.();
    });

    const key = host.querySelector<HTMLInputElement>(".cf-key");
    key?.addEventListener("input", () => {
      // Never touched programmatically: a masked saved key round-trips intact
      // unless the user themselves replaces it.
      opts.setApiKey(key.value);
      clearTest();
    });

    const keyLink = host.querySelector<HTMLAnchorElement>(".cf-key-link");
    keyLink?.addEventListener("click", async (e) => {
      // Tauri webviews don't follow target=_blank on their own — route the
      // anchor through the shell plugin so it lands in the system browser.
      e.preventDefault();
      const { open } = await import("@tauri-apps/plugin-shell");
      await open(keyLink.href).catch(() => {});
    });

    const url = host.querySelector<HTMLInputElement>(".cf-url");
    url?.addEventListener("input", () => {
      opts.setApiUrl(url.value);
      clearTest();
      // Editing the endpoint can re-name the connection (e.g. pasting the Groq
      // URL) or un-name it (→ Custom). Move the select marker — and its hint —
      // without re-rendering, so the input keeps focus.
      if (!choseCustom) {
        const id = selectedId();
        const select = host.querySelector<HTMLSelectElement>(".cf-provider");
        if (select && select.value !== id) {
          select.value = id;
          const hintEl = host.querySelector<HTMLElement>(".cf-provider-hint");
          if (hintEl) hintEl.textContent = byId.get(id)?.hint ?? "";
        }
      }
    });

    const details = host.querySelector<HTMLDetailsElement>(".cf-advanced");
    details?.addEventListener("toggle", () => {
      advancedOpen = details.open;
    });

    host.querySelector<HTMLButtonElement>(".cf-test")?.addEventListener("click", () => void runTest());
  };

  render();
}
