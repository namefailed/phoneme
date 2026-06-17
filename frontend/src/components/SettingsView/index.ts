import { escapeHtml } from "../../utils/format";
import { errText } from "../../utils/error";
import { LitElement, html } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../../utils/toast";
import { fuzzyScore } from "../../utils/fuzzy";
import { keywordsForKey } from "./searchKeywords";
import { getSettingsAnchor } from "../shared/settingsAnchor";

import { SectionWhisper } from "./SectionWhisper";
import { SectionPreview } from "./SectionPreview";
import { SectionDiarization } from "./SectionDiarization";
import { SectionRecording } from "./SectionRecording";
import { SectionHotkeys } from "./SectionHotkeys";
import { SectionHook } from "./SectionHook";
import { SectionStorage } from "./SectionStorage";
import { SectionSemantic } from "./SectionSemantic";
import { SectionTray } from "./SectionTray";
import { SectionInterface } from "./SectionInterface";
import { SectionPostProcessing } from "./SectionPostProcessing";
import { SectionEditor } from "./SectionEditor";
import { SectionAdvanced } from "./SectionAdvanced";
import { SectionTags } from "./SectionTags";
import { SectionProfiles } from "./SectionProfiles";
import { SectionSavedSearches } from "./SectionSavedSearches";
import { SectionAutoTag } from "./SectionAutoTag";
import { SectionInPlace } from "./SectionInPlace";
import { SectionIntegrations } from "./SectionIntegrations";
import "./styles.css";

// ── Settings-search helpers ────────────────────────────────────────────────

/** Escape text for safe innerHTML insertion. */

/** Wrap the first contiguous, case-insensitive run of `query` in `original` with
 *  a <mark>. Returns the escaped original unchanged when there's no contiguous
 *  run (e.g. a fuzzy- or keyword-only match), so a highlight never lies about
 *  why a result appeared. */
function highlightText(original: string, query: string): string {
  const q = query.trim();
  if (!q) return escapeHtml(original);
  const i = original.toLowerCase().indexOf(q.toLowerCase());
  if (i < 0) return escapeHtml(original);
  return (
    escapeHtml(original.slice(0, i)) +
    `<mark class="settings-hit">${escapeHtml(original.slice(i, i + q.length))}</mark>` +
    escapeHtml(original.slice(i + q.length))
  );
}

/** Read (and cache once, on the element's dataset) its pristine text, so every
 *  re-highlight starts from the un-marked original rather than compounding. Also
 *  records — at pristine time, before any <mark> is injected — whether the node
 *  is plain text and therefore safe to highlight (svHl === "1"); a later check
 *  of `children.length` would be wrong once the first highlight adds a <mark>. */
function origText(el: HTMLElement | null): string | null {
  if (!el) return null;
  if (el.dataset.svOrig === undefined) {
    el.dataset.svOrig = el.textContent ?? "";
    el.dataset.svHl = el.children.length === 0 ? "1" : "0";
  }
  return el.dataset.svOrig;
}

/** Best fuzzy score of `query` across a field's intent keywords (null = none). */
function bestKeywordScore(query: string, words: string[]): number | null {
  let best: number | null = null;
  for (const w of words) {
    const s = fuzzyScore(query, w);
    if (s !== null && (best === null || s > best)) best = s;
  }
  return best;
}

/** The Settings tab rail, in display order. A single source of truth that drives
 *  the sidebar buttons, the ⚙ float-menu jump list, and (via the section
 *  registry's `tab` field) which sections mount under each tab. The trio at the
 *  top mirrors the transcription pipeline; the heavier groups (Recall, System)
 *  are their own tabs rather than one overloaded catch-all. */
// Ordered by a recording's journey: how it's captured → transcribed → enriched →
// found, then (in render) the managers, then the app-level tabs (Appearance,
// System) last. Appearance + System are pulled to the end of the rail in
// render() so they sit together after the managers.
const SETTINGS_TABS: { id: string; label: string }[] = [
  { id: "capture", label: "🎙️ Capture" },
  { id: "dictation", label: "⌨️ Dictation" },
  { id: "transcription", label: "🗣️ Transcription" },
  { id: "preview", label: "👁️ Live Preview" },
  { id: "diarization", label: "👥 Diarization" },
  { id: "postprocessing", label: "✨ Post-Processing" },
  { id: "recall", label: "🔮 Recall" },
  { id: "appearance", label: "🎨 Appearance" },
  { id: "system", label: "⚙️ System" },
];

/** The interactive manager surfaces — each is its own entry in the rail under a
 *  "Managers" group header, NOT a config tab (CRUD with side effects vs. the
 *  config-bound fields above). Ids are `managers/<x>` so the existing g-chord /
 *  header deep-links ("managers/profiles", "managers/saved") resolve straight
 *  here. Rendered between Appearance and System (see the rail in render()). */
const MANAGERS: { id: string; label: string }[] = [
  { id: "managers/tags", label: "🏷️ Tags" },
  { id: "managers/saved", label: "📌 Saved searches" },
  { id: "managers/profiles", label: "👤 Profiles" },
  { id: "managers/hooks", label: "🪝 Integrations" },
  { id: "managers/keybinds", label: "⚡ Keybinds" },
];

/** Legacy deep-link ids → current ids, so openers that predate the re-taxonomy
 *  (header jump-menu, g-chords, the Re-run "enable cleanup" link, saved links)
 *  keep resolving. `ai` was a brief rename of Post-Processing; the bare
 *  `managers`/`tags`/… ids now point at the corresponding rail entry. */
const LEGACY_TAB_ALIASES: Record<string, string> = {
  ai: "postprocessing",
  managers: "managers/tags",
  tags: "managers/tags",
  profiles: "managers/profiles",
  saved: "managers/saved",
};
function resolveTab(rawTab: string): string {
  return LEGACY_TAB_ALIASES[rawTab] ?? rawTab;
}

/**
 * The Settings view (the "settings" route): a tab rail + one mounted section
 * per tab, a fuzzy settings search (with per-field intent keywords from
 * searchKeywords.ts, ↑/↓ result navigation, and in-place filtering of the
 * mounted sections), and the floating ⚙/Save controls (the ⚙ snaps to where
 * the header button was, via shared/settingsAnchor).
 *
 * The config-editing contract every section participates in: this view loads
 * ONE mutable `config` object (`read_config`) and hands the SAME reference to
 * each section it mounts; sections bind their inputs to dotted config paths
 * (see form.ts) and mutate that object in place as the user types. Nothing
 * persists until Save, which `write_config`s the whole object, dispatches
 * `config:saved` (detail = the config) so live listeners re-apply (theme,
 * keyboard, list columns, sections re-mount), and closes the view.
 *
 * Unsaved-edits guard: `confirmClose()` (themed dialog) compares the JSON
 * snapshot taken at load — EVERY leave path App controls funnels through it.
 * Deep links: `activeTab` may arrive from openers ("post_processing",
 * "managers/profiles", …) via the `phoneme:navigate` event's section field.
 * Mounted by App via the `SettingsView` wrapper; the header bar is hidden
 * while this view is up.
 */
@customElement('ph-settings-view')
export class SettingsViewElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for global CSS (settings-layout, sv-tab, etc)
  }

  @property({ type: Object }) onClose!: () => void;
  @property({ type: Function }) onNavigateToWizard?: () => void;

  // Public so an opener (e.g. the Re-run "Enable cleanup in Settings" shortcut)
  // can deep-link to a tab; also mutated internally by switchTab.
  @property({ type: String }) activeTab: string = "transcription";
  @state() private config: any = null;
  @state() private searchQuery: string = "";
  /** In-panel ⚙ Settings split-button dropdown (mirrors the header's) (L). */
  @state() private floatMenuOpen = false;
  private originalConfigStr: string = "";
  /** Cursor index into the visible result fields for ↑/↓ keyboard nav. */
  private searchCursor = -1;

  @query('#settings-body') bodyEl!: HTMLElement;

  private onConfigSaved = (e: Event) => {
    const detail = (e as CustomEvent).detail;
    if (!detail) return;
    this.config = detail;
    this.originalConfigStr = JSON.stringify(this.config);
    this.mountSection();
  };

  async connectedCallback() {
    super.connectedCallback();
    document.addEventListener("keydown", this.onEscapeKey);
    try {
      this.config = await invoke("read_config");
      this.originalConfigStr = JSON.stringify(this.config);
      window.addEventListener("config:saved", this.onConfigSaved);
    } catch (e) {
      console.error(e);
      showToast(`Failed to load settings: ${errText(e)}`, "error");
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.onEscapeKey);
    window.removeEventListener("config:saved", this.onConfigSaved);
    document.removeEventListener("mousedown", this.onFloatOutside, true);
  }

  /** Escape leaves the Settings panel (with the unsaved-edits guard). Inner
   *  contexts that own Escape — the search box, open dropdowns, a layered modal
   *  like the unsaved-changes confirm — consume it first (stopPropagation / the
   *  `.modal-overlay` check below), so this only fires for a bare Escape in the
   *  panel. Keyboard nav INSIDE Settings is intentionally not wired yet; this is
   *  just the way out. */
  private onEscapeKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    if (this.floatMenuOpen) { this.closeFloatMenu(); return; }
    // A modal layered above Settings owns Escape (e.g. a model picker, the
    // unsaved-changes dialog). Don't close the panel out from under it.
    if (document.querySelector('[class*="modal-overlay"]')) return;
    // Escape inside an active field deselects/blurs THAT control — it must not
    // boot the user out of Settings mid-edit (only the search box stops its own
    // Escape; every other section field bubbles here). A second Escape, now off
    // the field, then leaves the panel as usual.
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) {
      t.blur();
      return;
    }
    e.preventDefault();
    void this.handleClose();
  };

  private searchDebounce?: ReturnType<typeof setTimeout>;

  protected updated(changedProperties: Map<string, any>) {
    const prevQuery: string = changedProperties.get('searchQuery') ?? "";
    const wasSearching = prevQuery.trim().length > 0;
    const isSearching = this.searchQuery.trim().length > 0;

    // Full (re)mount only when the tab or config changes, or when we ENTER/EXIT
    // search mode. While already searching, a query change just re-filters the
    // already-mounted sections in place — no teardown/rebuild — so typing is
    // instant and doesn't flash between the previous tab and the results.
    if (
      changedProperties.has('activeTab') ||
      changedProperties.has('config') ||
      (changedProperties.has('searchQuery') && wasSearching !== isSearching)
    ) {
      this.mountSection();
    } else if (changedProperties.has('searchQuery') && isSearching) {
      this.applySearchFilter();
    }
  }

  /** Whether the in-memory config differs from what was last loaded or saved. */
  private hasUnsavedChanges(): boolean {
    return !!this.config && JSON.stringify(this.config) !== this.originalConfigStr;
  }

  /**
   * Legacy sync gate (native confirm). Retained for the unit tests and any
   * caller needing a synchronous answer; the app now prefers {@link confirmClose}.
   */
  public canClose(): boolean {
    if (this.hasUnsavedChanges()) {
      return confirm("You have unsaved changes. Discard them?");
    }
    return true;
  }

  /**
   * Themed async gate for leaving Settings with unsaved edits: resolves `true`
   * to proceed (discard), `false` to stay. A no-op (`true`) when nothing is
   * unsaved, so callers can always await it before navigating away.
   */
  public async confirmClose(): Promise<boolean> {
    if (!this.hasUnsavedChanges()) return true;
    const { confirmDialog } = await import("../confirmDialog");
    return confirmDialog({
      title: "Discard unsaved changes?",
      body: "You've changed settings that haven't been saved yet. Leave without saving?",
      confirmLabel: "Discard",
      cancelLabel: "Keep editing",
      danger: true,
    });
  }

  private async handleClose() {
    if (await this.confirmClose()) this.onClose();
  }

  /** Inline position for the floating ⚙ Settings button: snap it to exactly
   *  where the header button was (captured on open) so opening Settings doesn't
   *  move it. Empty string → fall back to the CSS default (Settings opened via
   *  a keyboard shortcut or deep link, with no captured anchor). */
  private floatAnchorStyle(): string {
    const a = getSettingsAnchor();
    // Match the header button's exact pixel height too, not just its position.
    // The header ⚙ takes its height from the header-bar row (which doesn't grow
    // with the UI font size), whereas the float button would otherwise size to
    // its own font + padding and end up taller off the default font size. Pin
    // the group to the captured height; `.anchored` drops the children's vertical
    // padding so they stretch to fill it (see styles.css).
    return a
      ? `position: fixed; top: ${a.top}px; left: ${a.left}px; right: auto; height: ${a.height}px;`
      : "";
  }

  /** Inline position for the app-health pill: the far-right-most control, pinned
   *  14px from the window's right edge and vertically aligned to the captured
   *  ⚙-button band — the SAME screen spot (and the same button→pill / pill→edge
   *  gaps) as the header's pill, so the two views are identical. */
  private healthPillStyle(): string {
    const a = getSettingsAnchor();
    return a
      ? `position: fixed; top: ${a.top}px; height: ${a.height}px; right: 14px;`
      : `position: absolute; top: 18px; height: 32px; right: 14px;`;
  }

  /** Pin the toggle to the header ⚙ button's captured width so its "← Go Back"
   *  label (shorter than the header's "⚙ Settings") doesn't shrink the split
   *  button or shift the caret/pill. Centre the shorter label in that fixed box. */
  private floatToggleStyle(): string {
    const a = getSettingsAnchor();
    return a ? `box-sizing: border-box; width: ${a.width}px; justify-content: center;` : "";
  }

  private async handleSave() {
    try {
      if (this.config.hook) {
        if (this.config.hook.command !== undefined) {
          if (!Array.isArray(this.config.hook.commands)) {
            this.config.hook.commands = [this.config.hook.command];
          }
          delete this.config.hook.command;
        }
        if (Array.isArray(this.config.hook.commands)) {
          this.config.hook.commands = this.config.hook.commands
            .map((c: unknown) => String(c ?? ""))
            .filter((c: string) => c.trim() !== "");
        }
      }
      await invoke("write_config", { config: this.config });
      window.dispatchEvent(new CustomEvent("config:saved", { detail: this.config }));
      showToast("Settings saved", "success");
      this.onClose();
    } catch (e) {
      showToast(`Save failed: ${errText(e)}`, "error");
    }
  }

  private mountSection() {
    if (!this.bodyEl || !this.config) return;
    
    this.bodyEl.innerHTML = "";
    const sectionHost = document.createElement("div");
    this.bodyEl.appendChild(sectionHost);

    const isSearching = this.searchQuery.trim().length > 0;

    const createSubHost = () => {
      const subHost = document.createElement("div");
      sectionHost.appendChild(subHost);
      return subHost;
    };

    if (isSearching) {
      // Search mounts EVERY section once, each in its own tab-tagged host so a
      // result can show which tab it lives in and offer a jump there. Later
      // keystrokes only re-filter in place (see updated()), so typing stays
      // instant and flicker-free.
      const header = document.createElement("div");
      header.id = "settings-search-header";
      sectionHost.appendChild(header);
      for (const s of this.sectionRegistry()) {
        const host = document.createElement("div");
        host.className = "sv-result-host";
        host.dataset.tab = s.tab;
        host.dataset.tabLabel = s.label;
        sectionHost.appendChild(host);
        s.mount(host);
      }
      this.applySearchFilter();
    } else {
      // Config tab OR a manager id ("managers/hooks") — both are registry-driven
      // now (managers are their own rail entries, no sub-tab strip). Mount, in
      // order, each section whose `tab` matches. One source of truth with search.
      const tab = resolveTab(this.activeTab);
      for (const s of this.sectionRegistry()) {
        if (s.tab === tab) s.mount(createSubHost());
      }
    }
  }

  /** The single source of truth for which sections exist and which tab each
   *  belongs to (see {@link SETTINGS_TABS} for the tab order/labels). Drives
   *  both the search index (all sections mounted at once) and per-tab rendering
   *  (filtered by `tab`). `label` is the breadcrumb tab name shown on a search
   *  result. Managers' three sections are listed here so they're individually
   *  searchable, but the Managers TAB renders its own sub-tab strip instead. */
  private sectionRegistry(): { tab: string; label: string; mount: (h: HTMLElement) => void }[] {
    const c = this.config;
    return [
      { tab: "transcription", label: "Transcription", mount: (h) => { new SectionWhisper(h, c); } },
      { tab: "preview", label: "Live Preview", mount: (h) => { new SectionPreview(h, c); } },
      { tab: "diarization", label: "Diarization", mount: (h) => { new SectionDiarization(h, c); } },
      { tab: "capture", label: "Capture", mount: (h) => { new SectionRecording(h, c); } },
      { tab: "dictation", label: "Dictation", mount: (h) => { new SectionInPlace(h, c); } },
      { tab: "postprocessing", label: "Post-Processing", mount: (h) => { new SectionPostProcessing(h, c); } },
      { tab: "postprocessing", label: "Post-Processing", mount: (h) => { new SectionAutoTag(h, c); } },
      { tab: "recall", label: "Recall", mount: (h) => { new SectionSemantic(h, c); } },
      { tab: "appearance", label: "Appearance", mount: (h) => { new SectionInterface(h, c); } },
      { tab: "appearance", label: "Appearance", mount: (h) => { new SectionEditor(h, c); } },
      // Managers (each its own rail entry under the "Managers" group).
      { tab: "managers/tags", label: "Tags", mount: (h) => { new SectionTags(h, c); } },
      { tab: "managers/profiles", label: "Profiles", mount: (h) => { new SectionProfiles(h, c); } },
      { tab: "managers/saved", label: "Saved searches", mount: (h) => { new SectionSavedSearches(h, c); } },
      // Hook Manager — outbound (scripts + webhook) AND inbound (REST + MCP)
      // automation in one place.
      { tab: "managers/hooks", label: "Integrations", mount: (h) => { new SectionHook(h, c); } },
      { tab: "managers/hooks", label: "Integrations", mount: (h) => { new SectionIntegrations(h, c); } },
      { tab: "managers/keybinds", label: "Keybinds", mount: (h) => { new SectionHotkeys(h, c); } },
      { tab: "system", label: "System", mount: (h) => { new SectionStorage(h, c); } },
      { tab: "system", label: "System", mount: (h) => { new SectionTray(h, c); } },
      { tab: "system", label: "System", mount: (h) => { new SectionAdvanced(h, c, this.onNavigateToWizard); } },
    ];
  }

  /**
   * Filter the already-mounted search results in place. For every field we score
   * the query against (in priority order) its label, its intent keywords (see
   * searchKeywords.ts — this is what makes "dark"/"password"/"shortcut" land),
   * and its description; the section title also counts. Matches are highlighted,
   * sections are ordered most-relevant first, and each keeps a breadcrumb back
   * to its home tab. Called on every (debounced) keystroke — no remount, so it
   * stays instant and flicker-free.
   */
  private applySearchFilter() {
    if (!this.bodyEl) return;
    // Visibility/order is about to change — reset the ↑/↓ result cursor.
    this.searchCursor = -1;
    this.bodyEl.querySelectorAll(".sv-result-active").forEach((el) => el.classList.remove("sv-result-active"));
    const raw = this.searchQuery.trim();
    const query = raw.toLowerCase();
    const hosts = [...this.bodyEl.querySelectorAll<HTMLElement>(".sv-result-host")];
    let visibleSections = 0;
    let visibleFields = 0;
    const ranked: { host: HTMLElement; score: number }[] = [];

    for (const host of hosts) {
      const sec = host.querySelector<HTMLElement>(".settings-section");
      if (!sec) {
        host.style.display = "none";
        continue;
      }
      const h3 = sec.querySelector<HTMLElement>("h3");
      const titleOrig = origText(h3) ?? "";
      const titleScore = titleOrig ? fuzzyScore(query, titleOrig) : null;
      const titleMatched = titleScore !== null;

      const fields = [...sec.querySelectorAll<HTMLElement>(".settings-field")];
      let bestScore = titleMatched ? 200 + (titleScore as number) : Number.NEGATIVE_INFINITY;
      let anyFieldHit = false;

      for (const field of fields) {
        const labelEl =
          field.querySelector<HTMLElement>(":scope > label") ?? field.querySelector<HTMLElement>("label");
        const labelOrig = origText(labelEl) ?? (field.textContent ?? "").trim().slice(0, 60);
        const keys = [...field.querySelectorAll<HTMLElement>("[data-key]")].map((e) => e.getAttribute("data-key") || "");
        const keywords = keys.flatMap(keywordsForKey);
        const fullText = (field.textContent ?? "").toLowerCase();

        const labelScore = fuzzyScore(query, labelOrig);
        const kwScore = bestKeywordScore(query, keywords);
        const descHit = query.length > 0 && fullText.includes(query);

        // Tiered scoring: a label hit beats a synonym hit beats a buried
        // description hit; a field with no hit of its own still shows when its
        // section title matched (ranked below real field hits).
        let score: number | null = null;
        if (labelScore !== null) score = 2000 + labelScore;
        if (kwScore !== null) score = Math.max(score ?? Number.NEGATIVE_INFINITY, 1200 + kwScore);
        if (descHit) score = Math.max(score ?? Number.NEGATIVE_INFINITY, 800);
        const ownHit = score !== null;
        if (score === null && titleMatched) score = 100 + (titleScore as number);

        if (score !== null) {
          field.style.display = "";
          visibleFields++;
          anyFieldHit = anyFieldHit || ownHit;
          bestScore = Math.max(bestScore, score);
          // Re-highlight from the pristine original every pass (a hidden field
          // simply rebuilds when it next reappears — no stale mark is ever seen).
          if (labelEl && labelEl.dataset.svHl === "1") labelEl.innerHTML = highlightText(labelOrig, raw);
        } else {
          field.style.display = "none";
        }
      }

      const visible = anyFieldHit || titleMatched;
      host.style.display = visible ? "" : "none";
      if (visible) {
        visibleSections++;
        if (h3 && h3.dataset.svHl === "1") h3.innerHTML = highlightText(titleOrig, raw);
        this.ensureBreadcrumb(host);
        ranked.push({ host, score: bestScore });
      }
    }

    // Most-relevant section first (stable for ties via the original DOM order).
    ranked.sort((a, b) => b.score - a.score).forEach((r) => r.host.parentElement?.appendChild(r.host));

    this.renderSearchHeader(visibleFields, visibleSections);
  }

  /** Add (once) a clickable "← which tab this lives in" chip atop a result host. */
  private ensureBreadcrumb(host: HTMLElement): void {
    if (host.querySelector(":scope > .sv-result-breadcrumb")) return;
    const tab = host.dataset.tab || "";
    const label = host.dataset.tabLabel || "";
    const sectionTitle = origText(host.querySelector<HTMLElement>("h3")) ?? "";
    const bc = document.createElement("button");
    bc.type = "button";
    bc.className = "sv-result-breadcrumb";
    bc.title = `Open in ${label} settings`;
    bc.innerHTML =
      `<span class="sv-bc-tab">${escapeHtml(label)}</span>` +
      `<span class="sv-bc-go" aria-hidden="true">Open ↗</span>`;
    bc.addEventListener("click", () => this.jumpToSection(tab, sectionTitle));
    host.insertBefore(bc, host.firstChild);
  }

  /** Leave search and open the given tab, scrolling its section into view with
   *  a brief highlight so the setting the user clicked is easy to spot. */
  private jumpToSection(tab: string, sectionTitle: string): void {
    if (this.searchDebounce) clearTimeout(this.searchDebounce);
    this.activeTab = tab;
    this.searchQuery = "";
    const input = this.renderRoot.querySelector(".settings-search") as HTMLInputElement | null;
    if (input) input.value = "";
    void this.updateComplete.then(() => {
      requestAnimationFrame(() => {
        const secs = this.bodyEl?.querySelectorAll<HTMLElement>(".settings-section") ?? [];
        secs.forEach((s) => {
          if ((s.querySelector("h3")?.textContent ?? "").trim() === sectionTitle.trim()) {
            s.scrollIntoView({ behavior: "smooth", block: "start" });
            s.classList.add("sv-flash");
            setTimeout(() => s.classList.remove("sv-flash"), 1200);
          }
        });
      });
    });
  }

  /** Update the results bar above the search hits (count, or a friendly empty state). */
  private renderSearchHeader(fields: number, sections: number): void {
    const header = this.bodyEl?.querySelector<HTMLElement>("#settings-search-header");
    if (!header) return;
    const q = escapeHtml(this.searchQuery.trim());
    if (fields === 0) {
      header.className = "sv-search-empty";
      header.innerHTML =
        `<div class="sv-empty-icon">🔍</div>` +
        `<div class="sv-empty-title">No settings match “${q}”</div>` +
        `<div class="sv-empty-hint">Try a feature name like “theme”, “shortcut”, “microphone”, or “api key”.</div>`;
    } else {
      header.className = "sv-search-count";
      header.innerHTML =
        `<svg class="sv-count-ico" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>` +
        `<span><strong>${fields}</strong> setting${fields === 1 ? "" : "s"} in ` +
        `<strong>${sections}</strong> section${sections === 1 ? "" : "s"} match “${q}”</span>`;
    }
  }

  private switchTab(tab: string) {
    if (this.activeTab !== tab) {
      this.activeTab = tab;
      this.searchQuery = "";
      const searchInput = this.renderRoot.querySelector('.settings-search') as HTMLInputElement;
      if (searchInput) searchInput.value = "";
    }
  }

  // ── In-panel ⚙ Settings split-button dropdown (L) ─────────────────────────
  // Mirrors the header's quick-settings menu (HeaderBar) so the button behaves
  // the same inside the Settings view: main half closes; the caret opens a
  // jump-to-section + Quick-model-switch menu. "Jump" switches the tab here
  // (we're already in Settings) rather than firing a navigate event.
  private toggleFloatMenu = (e: Event) => {
    e.stopPropagation();
    this.floatMenuOpen = !this.floatMenuOpen;
    if (this.floatMenuOpen) document.addEventListener("mousedown", this.onFloatOutside, true);
    else document.removeEventListener("mousedown", this.onFloatOutside, true);
  };

  private onFloatOutside = (e: MouseEvent) => {
    const grp = this.renderRoot.querySelector(".settings-float-group");
    if (grp && !grp.contains(e.target as Node)) this.closeFloatMenu();
  };

  private closeFloatMenu() {
    this.floatMenuOpen = false;
    document.removeEventListener("mousedown", this.onFloatOutside, true);
  }

  private jumpFloat(tab: string) {
    this.closeFloatMenu();
    this.switchTab(tab);
  }

  private openFloatModels = async () => {
    this.closeFloatMenu();
    const { openModelPicker } = await import("../ModelPicker");
    await openModelPicker("transcription");
  };

  private handleSearch(e: Event) {
    const value = (e.target as HTMLInputElement).value;
    // Debounce so each keystroke doesn't trigger a reactive update; ~140ms
    // keeps typing snappy while collapsing rapid input into one filter pass.
    if (this.searchDebounce) clearTimeout(this.searchDebounce);
    this.searchDebounce = setTimeout(() => {
      this.searchQuery = value;
    }, 140);
  }

  /** Keyboard-drive the results without leaving the search box:
   *  Esc clears (or blurs), ↑/↓ step through the live results, Enter drops
   *  focus into the highlighted field's control so it can be edited. */
  private handleSearchKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      const input = e.target as HTMLInputElement;
      if (input.value || this.searchQuery) {
        e.preventDefault();
        this.clearSearch();
      } else {
        input.blur();
      }
      return;
    }
    if (!this.searchQuery.trim()) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      this.moveResultCursor(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      this.moveResultCursor(-1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const field = this.visibleResultFields()[this.searchCursor];
      field?.querySelector<HTMLElement>("input, select, textarea, button")?.focus();
    }
  }

  /** Result fields currently on screen, in displayed (relevance) order. */
  private visibleResultFields(): HTMLElement[] {
    const all = this.bodyEl?.querySelectorAll<HTMLElement>(".sv-result-host .settings-field");
    return all ? [...all].filter((f) => f.offsetParent !== null) : [];
  }

  private moveResultCursor(delta: number) {
    const fields = this.visibleResultFields();
    if (!fields.length) return;
    fields.forEach((f) => f.classList.remove("sv-result-active"));
    this.searchCursor = Math.max(0, Math.min(fields.length - 1, this.searchCursor + delta));
    const field = fields[this.searchCursor];
    field.classList.add("sv-result-active");
    field.scrollIntoView({ block: "nearest" });
  }

  private clearSearch() {
    if (this.searchDebounce) clearTimeout(this.searchDebounce);
    this.searchQuery = "";
    const input = this.renderRoot.querySelector(".settings-search") as HTMLInputElement | null;
    if (input) {
      input.value = "";
      input.focus();
    }
  }

  /** Focus the search box on open so the keyboard is immediately useful. */
  protected firstUpdated() {
    (this.renderRoot.querySelector(".settings-search") as HTMLInputElement | null)?.focus();
  }

  render() {
    if (!this.config) {
      return html`<div class="error">Loading settings...</div>`;
    }

    const isSearching = this.searchQuery.trim().length > 0;
    // The active tab is a config id, a manager id ("managers/hooks"), or a
    // legacy id ("ai"/"tags") — resolve it to the current id for highlighting.
    const tab = resolveTab(this.activeTab);

    return html`
      <div class="settings-layout">
        <div class="settings-sidebar">
          <h2>Settings</h2>
          <div class="sv-search-wrap">
            <svg class="sv-search-ico" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>
            <input type="search" class="settings-search" placeholder="Search settings…" @input=${this.handleSearch} @keydown=${this.handleSearchKeydown} />
            <button type="button" class="sv-search-clear ${isSearching ? "" : "is-hidden"}" title="Clear search (Esc)" aria-label="Clear search" @click=${this.clearSearch}>✕</button>
          </div>

          ${(() => {
            // All tabs render flat in the rail: the config tabs, then each
            // manager (Tags · Profiles · … · Keybinds) as its own plain tab, then
            // System last (see SETTINGS_TABS / MANAGERS).
            const chip = (id: string, label: string) => html`<div
              class="sv-tab ${tab === id && !isSearching ? "active" : ""}"
              @click=${() => this.switchTab(id)}
            >${label}</div>`;
            // Pipeline tabs first, then the managers, then the app-level tabs
            // (Appearance, System) last — grouped at the bottom after the managers.
            const APP_TABS = ["appearance", "system"];
            const appTabs = APP_TABS.map((id) => SETTINGS_TABS.find((t) => t.id === id)).filter(Boolean) as { id: string; label: string }[];
            return html`
              ${SETTINGS_TABS.filter((t) => !APP_TABS.includes(t.id)).map((t) => chip(t.id, t.label))}
              ${MANAGERS.map((m) => chip(m.id, m.label))}
              ${appTabs.map((t) => chip(t.id, t.label))}
            `;
          })()}

          ${isSearching ? html`<div class="sv-tab active" style="margin-top: 12px; font-style: italic;">Search Results</div>` : ""}
        </div>
        <div class="settings-main">
          <div class="settings-float-group ${getSettingsAnchor() ? "anchored" : ""}" style=${this.floatAnchorStyle()}>
            <button class="settings-float-toggle" style=${this.floatToggleStyle()} title="Go back" aria-label="Go back" @click=${this.handleClose}>← Go Back</button>
            <button class="settings-float-caret ${this.floatMenuOpen ? "active" : ""}" aria-label="Quick settings &amp; actions" aria-haspopup="menu" aria-expanded=${this.floatMenuOpen} title="Quick settings &amp; actions" @click=${this.toggleFloatMenu}>
              <svg class="ph-caret-ico ${this.floatMenuOpen ? "open" : ""}" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg>
            </button>
            <div class="hb-settings-menu" role="menu" ?hidden=${!this.floatMenuOpen}
              style="position:absolute; top:calc(100% + 6px); right:0; z-index:60; min-width:230px; background:var(--bg-elevated, #1e1e2e); border:var(--popup-border, 1px solid rgba(255,255,255,0.12)); border-radius:10px; padding:5px; box-shadow:0 10px 30px rgba(0,0,0,0.5);">
              <button class="hb-menu-item" role="menuitem" @click=${this.openFloatModels}><span class="hb-menu-ico">🎛</span>Quick model switch…</button>
              <div class="hb-menu-sep"></div>
              <div class="hb-menu-label">Jump to section</div>
              ${[
                ...SETTINGS_TABS.filter((t) => t.id !== "appearance" && t.id !== "system"),
                ...MANAGERS,
                ...SETTINGS_TABS.filter((t) => t.id === "appearance" || t.id === "system"),
              ].map((t) => {
                // Split the leading emoji off the label so it sits in the icon slot.
                const sp = t.label.indexOf(" ");
                const ico = sp > 0 ? t.label.slice(0, sp) : "";
                const name = sp > 0 ? t.label.slice(sp + 1) : t.label;
                return html`<button class="hb-menu-item" role="menuitem" @click=${() => this.jumpFloat(t.id)}><span class="hb-menu-ico">${ico}</span>${name}</button>`;
              })}
            </div>
          </div>
          <!-- The app-health pill, placed at the exact same screen spot as the
               header's far-right one: pinned 14px from the window edge, aligned to
               the captured ⚙-button band — so both views look identical. Kept OUT
               of the float group so it never shifts the ⚙ button off its anchor. -->
          <ph-health-pill class="sv-health" style=${this.healthPillStyle()}></ph-health-pill>
          <div class="settings-body" id="settings-body"></div>
          <div class="settings-float-actions">
            <button id="settings-close" @click=${this.handleClose}>Close</button>
            <button class="primary" id="settings-save" @click=${this.handleSave}>Save</button>
          </div>
        </div>
      </div>
    `;
  }
}

/** Imperative mount wrapper App uses for the settings route. Re-exposes the
 *  unsaved-edits gates (`canClose` sync/legacy, `confirmClose` themed async)
 *  that App's `tryNavigate` checks before leaving; `initialTab` deep-links a
 *  tab (or "managers/<sub>" composite) on mount. */
export class SettingsView {
  private element: SettingsViewElement;
  constructor(container: HTMLElement, onClose: () => void, onNavigateToWizard?: () => void, initialTab?: string | null) {
    this.element = document.createElement('ph-settings-view') as SettingsViewElement;
    this.element.onClose = onClose;
    this.element.onNavigateToWizard = onNavigateToWizard;
    if (initialTab) this.element.activeTab = initialTab;
    container.appendChild(this.element);
  }

  public canClose(): boolean {
    return this.element.canClose();
  }

  public confirmClose(): Promise<boolean> {
    return this.element.confirmClose();
  }

  dispose() {
    this.element.remove();
  }
}
