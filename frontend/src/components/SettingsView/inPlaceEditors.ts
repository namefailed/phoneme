// The per-list editors for the Dictation settings section — each renders its
// rows + Add control into a host inside `container` and writes straight into
// the shared `config.in_place.*` table, re-rendering itself on every mutation
// (the same recurse-on-change pattern the methods used when they lived on
// SectionInPlace). Moved out verbatim; SectionInPlace now delegates to these.
import { escapeHtml as escHtml, escapeAttr } from "../../utils/format";
import {
  type PlaybookRecipe,
  listDictationHistory,
  regrabDictation,
  deleteDictationHistory,
  clearDictationHistory,
  type DictationHistoryRow,
} from "../../services/ipc";
import { showToast } from "../../utils/toast";

/** Must match the catalog's `DICTATION_HISTORY_KEEP` — only used for the copy in
 *  the opt-in toggle's description ("the last N dictations are kept"). */
export const DICTATION_HISTORY_KEEP = 50;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Config = any;

/** The recipes a per-app tone row can pick — `config.recipes` (treated as `[]`
 *  when absent), the same source SectionHotkeys offers a binding. */
function recipesOf(config: Config): PlaybookRecipe[] {
  return Array.isArray(config.recipes) ? (config.recipes as PlaybookRecipe[]) : [];
}

/** Build the recipe `<option>` list for a per-app tone select, mirroring
 *  SectionHotkeys' picker: every named recipe by id. Unlike the hotkey picker
 *  there is no empty "Default pipeline" choice — a per-app row only exists to
 *  name a specific recipe (remove the row to drop the override). A value that
 *  no longer names a live recipe keeps a visible "(missing)" option so a save
 *  never silently rewrites it; the daemon's `resolve_recipe` falls back to the
 *  default chain for a deleted id. */
export function recipeOptions(config: Config, selected: string): string {
  const opts: string[] = [];
  let matched = false;
  for (const r of recipesOf(config)) {
    const sel = r.id === selected;
    if (sel) matched = true;
    opts.push(
      `<option value="${escapeAttr(r.id)}" ${sel ? "selected" : ""}>${escHtml(r.name || r.id)}</option>`,
    );
  }
  // No recipes defined yet: offer a disabled hint so the control isn't an empty
  // box. The Add handler guards on an empty value, so nothing can be added.
  if (opts.length === 0) {
    return `<option value="" disabled selected>No recipes — create one in Playbook</option>`;
  }
  if (!matched && selected) {
    opts.push(
      `<option value="${escapeAttr(selected)}" selected>${escHtml(selected)} (missing)</option>`,
    );
  }
  return opts.join("");
}

/** Render the voice-command phrase→action rows + the Add control. Each row
 *  writes straight into `in_place.voice_commands` (phrase keys lowercased to
 *  match how the daemon compares command segments). An empty map means "use
 *  the built-in defaults", so the empty state says exactly that. Only present
 *  while voice commands are enabled (the section rebuilds when that flips). */
export function renderVoiceCommands(container: HTMLElement, config: Config) {
  const host = container.querySelector<HTMLElement>("#ip-voice-commands");
  if (!host) return;
  const map: Record<string, string> = config.in_place.voice_commands ?? {};
  const phrases = Object.keys(map).sort();
  // Human-facing label for each action value (the daemon's action strings).
  const actionLabel: Record<string, string> = {
    newline: "Line break",
    paragraph: "Blank line",
    scratch: "Scratch (drop last sentence)",
  };
  const optionFor = (cur: string) =>
    (["newline", "paragraph", "scratch"] as const)
      .map(
        (a) =>
          `<option value="${a}" ${cur === a ? "selected" : ""}>${escHtml(actionLabel[a])}</option>`,
      )
      .join("");
  host.innerHTML =
    phrases.length === 0
      ? `<span style="font-size: 0.7857rem; color: var(--fg-faded);">Using the built-in defaults — add a row to start a custom set (it replaces the defaults).</span>`
      : phrases
          .map(
            (phrase) => `
        <div class="ip-vc-row" data-phrase="${escapeAttr(phrase)}"
          style="display: flex; gap: 6px; width: 100%; align-items: center;">
          <span style="flex: 1 1 auto; min-width: 0; font-family: var(--font-mono, monospace); overflow: hidden; text-overflow: ellipsis;">${escHtml(phrase)}</span>
          <select class="ip-vc-action" data-phrase="${escapeAttr(phrase)}">${optionFor(map[phrase])}</select>
          <button class="ip-vc-remove" type="button" data-phrase="${escapeAttr(phrase)}" title="Remove">✕</button>
        </div>`,
          )
          .join("");

  host.querySelectorAll<HTMLSelectElement>(".ip-vc-action").forEach((sel) => {
    sel.addEventListener("change", () => {
      const phrase = sel.getAttribute("data-phrase");
      if (phrase) config.in_place.voice_commands[phrase] = sel.value;
    });
  });
  host.querySelectorAll<HTMLButtonElement>(".ip-vc-remove").forEach((btn) => {
    btn.addEventListener("click", () => {
      const phrase = btn.getAttribute("data-phrase");
      if (phrase) delete config.in_place.voice_commands[phrase];
      renderVoiceCommands(container, config);
    });
  });

  const phraseInput = container.querySelector<HTMLInputElement>("#ip-vc-add-phrase");
  const actionSel = container.querySelector<HTMLSelectElement>("#ip-vc-add-action");
  const addBtn = container.querySelector<HTMLButtonElement>("#ip-vc-add-btn");
  const add = () => {
    // Lowercase the phrase — the daemon compares against the lowercased,
    // connective-stripped command segment, so "New Line" / "new line" match.
    const raw = (phraseInput?.value ?? "").trim().toLowerCase();
    if (!raw) return;
    config.in_place.voice_commands[raw] = actionSel?.value ?? "newline";
    if (phraseInput) phraseInput.value = "";
    renderVoiceCommands(container, config);
  };
  addBtn?.addEventListener("click", add);
  phraseInput?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      add();
    }
  });
}

/** Render the text-macro (snippet) trigger→expansion rows + the Add control.
 *  Each row writes straight into `in_place.snippets` (trigger keys lowercased,
 *  matching how the daemon compares them case-insensitively). There is no
 *  built-in set, so the empty state says exactly that. Only present while
 *  snippets are enabled (the section rebuilds when that flips). */
export function renderSnippets(container: HTMLElement, config: Config) {
  const host = container.querySelector<HTMLElement>("#ip-snippets");
  if (!host) return;
  const map: Record<string, string> = config.in_place.snippets ?? {};
  const triggers = Object.keys(map).sort();
  host.innerHTML =
    triggers.length === 0
      ? `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No macros yet — add a trigger and its expansion to get started.</span>`
      : triggers
          .map(
            (trigger) => `
        <div class="ip-sn-row" data-trigger="${escapeAttr(trigger)}"
          style="display: flex; gap: 6px; width: 100%; align-items: center;">
          <span style="flex: 1 1 auto; min-width: 0; font-family: var(--font-mono, monospace); overflow: hidden; text-overflow: ellipsis;">${escHtml(trigger)}</span>
          <span style="flex: 0 0 auto; color: var(--fg-faded);">→</span>
          <input class="ip-sn-expansion" type="text" data-trigger="${escapeAttr(trigger)}"
            value="${escapeAttr(map[trigger])}" style="flex: 2 1 auto; min-width: 0;" />
          <button class="ip-sn-remove" type="button" data-trigger="${escapeAttr(trigger)}" title="Remove">✕</button>
        </div>`,
          )
          .join("");

  host.querySelectorAll<HTMLInputElement>(".ip-sn-expansion").forEach((inp) => {
    inp.addEventListener("input", () => {
      const trigger = inp.getAttribute("data-trigger");
      if (trigger) config.in_place.snippets[trigger] = inp.value;
    });
  });
  host.querySelectorAll<HTMLButtonElement>(".ip-sn-remove").forEach((btn) => {
    btn.addEventListener("click", () => {
      const trigger = btn.getAttribute("data-trigger");
      if (trigger) delete config.in_place.snippets[trigger];
      renderSnippets(container, config);
    });
  });

  const triggerInput = container.querySelector<HTMLInputElement>("#ip-sn-add-trigger");
  const expansionInput = container.querySelector<HTMLInputElement>("#ip-sn-add-expansion");
  const addBtn = container.querySelector<HTMLButtonElement>("#ip-sn-add-btn");
  const add = () => {
    // Lowercase the trigger — the daemon compares triggers case-insensitively,
    // and lowercasing keeps the stored key canonical (so "My Email" / "my email"
    // are one entry). The expansion is kept verbatim (its casing is the output).
    const trigger = (triggerInput?.value ?? "").trim().toLowerCase();
    const expansion = expansionInput?.value ?? "";
    if (!trigger) return;
    config.in_place.snippets[trigger] = expansion;
    if (triggerInput) triggerInput.value = "";
    if (expansionInput) expansionInput.value = "";
    renderSnippets(container, config);
  };
  addBtn?.addEventListener("click", add);
  // Enter in either Add field commits the row (only when a trigger is present).
  [triggerInput, expansionInput].forEach((el) =>
    el?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        add();
      }
    }),
  );
}

/** Render the per-app delivery rows (app name + mode + remove) and wire the
 *  Add control. Each row writes straight into `in_place.app_overrides`; the
 *  daemon keys it by the lowercased executable stem at typing time. */
export function renderAppOverrides(container: HTMLElement, config: Config) {
  const host = container.querySelector<HTMLElement>("#ip-app-overrides");
  if (!host) return;
  const overrides: Record<string, string> = config.in_place.app_overrides ?? {};
  const names = Object.keys(overrides).sort();
  host.innerHTML =
    names.length === 0
      ? `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No per-app overrides — every app uses the default above.</span>`
      : names
          .map(
            (name) => `
        <div class="ip-app-row" data-name="${escapeAttr(name)}"
          style="display: flex; gap: 6px; width: 100%; align-items: center;">
          <span style="flex: 1 1 auto; min-width: 0; font-family: var(--font-mono, monospace); overflow: hidden; text-overflow: ellipsis;">${escHtml(name)}</span>
          <select class="ip-app-mode" data-name="${escapeAttr(name)}">
            <option value="type" ${overrides[name] === "type" ? "selected" : ""}>Type</option>
            <option value="paste" ${overrides[name] === "paste" ? "selected" : ""}>Paste</option>
            <option value="off" ${overrides[name] === "off" ? "selected" : ""}>Off</option>
          </select>
          <button class="ip-app-remove" type="button" data-name="${escapeAttr(name)}" title="Remove">✕</button>
        </div>`,
          )
          .join("");

  host.querySelectorAll<HTMLSelectElement>(".ip-app-mode").forEach((sel) => {
    sel.addEventListener("change", () => {
      const name = sel.getAttribute("data-name");
      if (name) config.in_place.app_overrides[name] = sel.value;
    });
  });
  host.querySelectorAll<HTMLButtonElement>(".ip-app-remove").forEach((btn) => {
    btn.addEventListener("click", () => {
      const name = btn.getAttribute("data-name");
      if (name) delete config.in_place.app_overrides[name];
      renderAppOverrides(container, config);
    });
  });

  const nameInput = container.querySelector<HTMLInputElement>("#ip-app-add-name");
  const modeSel = container.querySelector<HTMLSelectElement>("#ip-app-add-mode");
  const addBtn = container.querySelector<HTMLButtonElement>("#ip-app-add-btn");
  const add = () => {
    // Store the stem lowercased — the daemon matches the focused process's
    // lowercased file stem, so "Code.exe" / "code" / "CODE" all normalize.
    const raw = (nameInput?.value ?? "").trim().replace(/\.exe$/i, "");
    if (!raw) return;
    config.in_place.app_overrides[raw.toLowerCase()] = modeSel?.value ?? "type";
    if (nameInput) nameInput.value = "";
    renderAppOverrides(container, config);
  };
  addBtn?.addEventListener("click", add);
  nameInput?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      add();
    }
  });
}

/** Render the per-app tone rows (app name + recipe picker + remove) and wire
 *  the Add control. Each row writes straight into `in_place.app_recipes`; the
 *  daemon keys it by the lowercased executable stem at record start and seeds
 *  the matching recipe into the pipeline. Mirrors `renderAppOverrides`, with a
 *  recipe `<select>` (from `config.recipes`) in place of the type/paste/off
 *  picker. */
export function renderAppRecipes(container: HTMLElement, config: Config) {
  const host = container.querySelector<HTMLElement>("#ip-app-recipes");
  if (!host) return;
  const map: Record<string, string> = config.in_place.app_recipes ?? {};
  const names = Object.keys(map).sort();
  host.innerHTML =
    names.length === 0
      ? `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No per-app tone — every app uses the default (or its hotkey's) recipe.</span>`
      : names
          .map(
            (name) => `
        <div class="ip-app-recipe-row" data-name="${escapeAttr(name)}"
          style="display: flex; gap: 6px; width: 100%; align-items: center;">
          <span style="flex: 1 1 auto; min-width: 0; font-family: var(--font-mono, monospace); overflow: hidden; text-overflow: ellipsis;">${escHtml(name)}</span>
          <select class="ip-app-recipe" data-name="${escapeAttr(name)}">${recipeOptions(config, map[name])}</select>
          <button class="ip-app-recipe-remove" type="button" data-name="${escapeAttr(name)}" title="Remove">✕</button>
        </div>`,
          )
          .join("");

  host.querySelectorAll<HTMLSelectElement>(".ip-app-recipe").forEach((sel) => {
    sel.addEventListener("change", () => {
      const name = sel.getAttribute("data-name");
      if (name) config.in_place.app_recipes[name] = sel.value;
    });
  });
  host.querySelectorAll<HTMLButtonElement>(".ip-app-recipe-remove").forEach((btn) => {
    btn.addEventListener("click", () => {
      const name = btn.getAttribute("data-name");
      if (name) delete config.in_place.app_recipes[name];
      renderAppRecipes(container, config);
    });
  });

  const nameInput = container.querySelector<HTMLInputElement>("#ip-app-recipe-add-name");
  const recipeSel = container.querySelector<HTMLSelectElement>("#ip-app-recipe-add-recipe");
  const addBtn = container.querySelector<HTMLButtonElement>("#ip-app-recipe-add-btn");
  const add = () => {
    // Store the stem lowercased — the daemon matches the focused process's
    // lowercased file stem, so "Outlook.exe" / "outlook" / "OUTLOOK" all match.
    const raw = (nameInput?.value ?? "").trim().replace(/\.exe$/i, "");
    const recipe = (recipeSel?.value ?? "").trim();
    // Both an app name and a recipe are required — an empty recipe would be a
    // no-op override (the resolver treats it as "no match"), so don't add it.
    if (!raw || !recipe) return;
    config.in_place.app_recipes[raw.toLowerCase()] = recipe;
    if (nameInput) nameInput.value = "";
    renderAppRecipes(container, config);
  };
  addBtn?.addEventListener("click", add);
  nameInput?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      add();
    }
  });
}

/** Render the app-context denylist rows + Add control. Only present while
 *  App-aware cleanup is on (the section is rebuilt when that flips). */
export function renderContextDenylist(container: HTMLElement, config: Config) {
  const host = container.querySelector<HTMLElement>("#ip-context-denylist");
  if (!host) return;
  const deny: string[] = config.in_place.app_context_denylist ?? [];
  host.innerHTML =
    deny.length === 0
      ? `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No apps excluded — titles may be read from any focused app.</span>`
      : deny
          .map(
            (name, i) => `
        <div style="display: flex; gap: 6px; width: 100%; align-items: center;">
          <span style="flex: 1 1 auto; min-width: 0; font-family: var(--font-mono, monospace); overflow: hidden; text-overflow: ellipsis;">${escHtml(name)}</span>
          <button class="ip-deny-remove" type="button" data-idx="${i}" title="Remove">✕</button>
        </div>`,
          )
          .join("");

  host.querySelectorAll<HTMLButtonElement>(".ip-deny-remove").forEach((btn) => {
    btn.addEventListener("click", () => {
      const idx = Number(btn.getAttribute("data-idx"));
      if (!Number.isNaN(idx)) config.in_place.app_context_denylist.splice(idx, 1);
      renderContextDenylist(container, config);
    });
  });

  const nameInput = container.querySelector<HTMLInputElement>("#ip-deny-add-name");
  const addBtn = container.querySelector<HTMLButtonElement>("#ip-deny-add-btn");
  const add = () => {
    const raw = (nameInput?.value ?? "").trim().replace(/\.exe$/i, "");
    if (!raw) return;
    const stem = raw.toLowerCase();
    if (!config.in_place.app_context_denylist.includes(stem)) {
      config.in_place.app_context_denylist.push(stem);
    }
    if (nameInput) nameInput.value = "";
    renderContextDenylist(container, config);
  };
  addBtn?.addEventListener("click", add);
  nameInput?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      add();
    }
  });
}

/** Best-effort friendly time for a stored dictation's UTC `created_at`. Falls
 *  back to the raw string if it doesn't parse. */
function formatHistoryTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

/** Fetch and render the recent-dictations list (newest first) with a Copy +
 *  Re-insert action per row, a per-row ✕, and wire the Clear-all button. Only
 *  present while `keep_history` is on (the section rebuilds when that flips).
 *
 *  NEEDS-NATIVE-VERIFY: Re-insert does real keystroke/paste injection through
 *  `regrabDictation`; the headless preview + mock can't exercise `type_at_cursor`,
 *  so the actual typing must be verified in the native window. */
export async function renderDictationHistory(container: HTMLElement) {
  const host = container.querySelector<HTMLElement>("#ip-dictation-history");
  if (!host) return;

  let rows: DictationHistoryRow[];
  try {
    rows = await listDictationHistory(DICTATION_HISTORY_KEEP);
  } catch {
    host.innerHTML = `<span style="font-size: 0.7857rem; color: var(--err);">Couldn't load dictation history (is the daemon running?).</span>`;
    return;
  }
  // The section can rebuild (e.g. the toggle flipped off) while the async fetch
  // was in flight — bail if our host was detached so we don't write into a
  // stale fragment.
  if (!host.isConnected) return;

  if (rows.length === 0) {
    host.innerHTML = `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No dictations yet — they'll appear here after you dictate with this on.</span>`;
    return;
  }

  host.innerHTML = rows
    .map((r) => {
      const when = formatHistoryTime(r.created_at);
      const appLabel = r.app ? ` · ${escHtml(r.app)}` : "";
      return `
        <div class="ip-dh-row" data-id="${r.id}"
          style="display: flex; gap: 6px; width: 100%; align-items: flex-start;">
          <div style="flex: 1 1 auto; min-width: 0; display: flex; flex-direction: column; gap: 2px;">
            <span style="overflow: hidden; text-overflow: ellipsis; white-space: nowrap;" title="${escHtml(r.text)}">${escHtml(r.text)}</span>
            <span style="font-size: 0.7143rem; color: var(--fg-faded);">${escHtml(when)} · ${r.char_count} chars${appLabel}</span>
          </div>
          <button class="ip-dh-copy" type="button" data-id="${r.id}" title="Copy to clipboard">Copy</button>
          <button class="ip-dh-regrab" type="button" data-id="${r.id}" title="Type this at the current cursor">Re-insert at cursor</button>
          <button class="ip-dh-remove" type="button" data-id="${r.id}" title="Forget">✕</button>
        </div>`;
    })
    .join("");

  const byId = new Map(rows.map((r) => [r.id, r] as const));
  const idOf = (btn: HTMLElement): number | null => {
    const v = Number(btn.getAttribute("data-id"));
    return Number.isNaN(v) ? null : v;
  };

  host.querySelectorAll<HTMLButtonElement>(".ip-dh-copy").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = idOf(btn);
      const row = id == null ? undefined : byId.get(id);
      if (!row) return;
      try {
        await navigator.clipboard.writeText(row.text);
        showToast("Dictation copied to clipboard", "success");
      } catch {
        showToast("Couldn't copy to clipboard", "error");
      }
    });
  });

  host.querySelectorAll<HTMLButtonElement>(".ip-dh-regrab").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = idOf(btn);
      if (id == null) return;
      try {
        await regrabDictation(id);
        showToast("Re-inserted at the cursor", "success");
      } catch {
        showToast("Couldn't re-insert the dictation", "error");
      }
    });
  });

  host.querySelectorAll<HTMLButtonElement>(".ip-dh-remove").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = idOf(btn);
      if (id == null) return;
      try {
        await deleteDictationHistory(id);
      } catch {
        showToast("Couldn't remove the dictation", "error");
        return;
      }
      await renderDictationHistory(container);
    });
  });

  const clearBtn = container.querySelector<HTMLButtonElement>("#ip-dh-clear");
  if (clearBtn && !clearBtn.dataset.wired) {
    clearBtn.dataset.wired = "1";
    clearBtn.addEventListener("click", async () => {
      try {
        await clearDictationHistory();
      } catch {
        showToast("Couldn't clear the history", "error");
        return;
      }
      await renderDictationHistory(container);
    });
  }
}
