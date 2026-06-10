import { errText } from "../../utils/error";
import { LitElement, html, css } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../../utils/toast";
import { fuzzyScore } from "../../utils/fuzzy";
import { keywordsForKey } from "./searchKeywords";

import { SectionWhisper } from "./SectionWhisper";
import { SectionPreview } from "./SectionPreview";
import { SectionDiarization } from "./SectionDiarization";
import { SectionRecording } from "./SectionRecording";
import { SectionHotkey } from "./SectionHotkey";
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
import "./styles.css";

// ── Settings-search helpers ────────────────────────────────────────────────

/** Escape text for safe innerHTML insertion. */
function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

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
    window.removeEventListener("config:saved", this.onConfigSaved);
  }

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

  public canClose(): boolean {
    if (this.config && JSON.stringify(this.config) !== this.originalConfigStr) {
      return confirm("You have unsaved changes. Discard them?");
    }
    return true;
  }

  private handleClose() {
    if (this.canClose()) this.onClose();
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

    // For search we mount EVERY section once, each in its own tab-tagged host so
    // a result can show which tab it lives in and offer a jump there. Later
    // keystrokes only re-filter in place (see updated()), so typing stays
    // instant and flicker-free. Order mirrors the per-tab layout below.
    const searchSections: { tab: string; label: string; mount: (h: HTMLElement) => void }[] = [
      { tab: "transcription", label: "Transcription", mount: (h) => { new SectionWhisper(h, this.config); } },
      { tab: "transcription", label: "Transcription", mount: (h) => { new SectionPreview(h, this.config); } },
      { tab: "transcription", label: "Transcription", mount: (h) => { new SectionDiarization(h, this.config); } },
      { tab: "capture", label: "Capture", mount: (h) => { new SectionRecording(h, this.config); } },
      { tab: "capture", label: "Capture", mount: (h) => { new SectionHotkey(h, this.config); } },
      { tab: "appearance", label: "Appearance", mount: (h) => { new SectionInterface(h, this.config); } },
      { tab: "appearance", label: "Appearance", mount: (h) => { new SectionEditor(h, this.config); } },
      { tab: "tags", label: "Tags", mount: (h) => { new SectionTags(h, this.config); } },
      { tab: "postprocessing", label: "Post-Processing", mount: (h) => { new SectionPostProcessing(h, this.config); } },
      { tab: "postprocessing", label: "Post-Processing", mount: (h) => { new SectionHook(h, this.config); } },
      { tab: "system", label: "System", mount: (h) => { new SectionStorage(h, this.config); } },
      { tab: "system", label: "System", mount: (h) => { new SectionSemantic(h, this.config); } },
      { tab: "system", label: "System", mount: (h) => { new SectionProfiles(h, this.config); } },
      { tab: "system", label: "System", mount: (h) => { new SectionTray(h, this.config); } },
      { tab: "system", label: "System", mount: (h) => { new SectionAdvanced(h, this.config, this.onNavigateToWizard); } },
    ];

    if (isSearching) {
      const header = document.createElement("div");
      header.id = "settings-search-header";
      sectionHost.appendChild(header);
      for (const s of searchSections) {
        const host = document.createElement("div");
        host.className = "sv-result-host";
        host.dataset.tab = s.tab;
        host.dataset.tabLabel = s.label;
        sectionHost.appendChild(host);
        s.mount(host);
      }
      this.applySearchFilter();
    } else {
      switch (this.activeTab) {
        case "transcription":
          new SectionWhisper(createSubHost(), this.config);
          // Live Preview sits directly under Whisper — it's a transcription
          // concern and was previously only reachable via search.
          new SectionPreview(createSubHost(), this.config);
          new SectionDiarization(createSubHost(), this.config);
          break;
        case "capture":
          new SectionRecording(createSubHost(), this.config);
          new SectionHotkey(createSubHost(), this.config);
          break;
        case "appearance":
          new SectionInterface(createSubHost(), this.config);
          new SectionEditor(createSubHost(), this.config);
          break;
        case "tags":
          new SectionTags(createSubHost(), this.config);
          break;
        case "postprocessing":
          new SectionPostProcessing(createSubHost(), this.config);
          new SectionHook(createSubHost(), this.config);
          break;
        case "system":
          new SectionStorage(createSubHost(), this.config);
          new SectionSemantic(createSubHost(), this.config);
          new SectionProfiles(createSubHost(), this.config);
          new SectionTray(createSubHost(), this.config);
          new SectionAdvanced(createSubHost(), this.config, this.onNavigateToWizard);
          break;
      }
    }
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
        `<strong>${fields}</strong> setting${fields === 1 ? "" : "s"} in ` +
        `<strong>${sections}</strong> section${sections === 1 ? "" : "s"} match “${q}”`;
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

    return html`
      <div class="settings-layout">
        <div class="settings-sidebar">
          <h2>Settings</h2>
          <div class="sv-search-wrap">
            <svg class="sv-search-ico" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>
            <input type="search" class="settings-search" placeholder="Search all settings…" @input=${this.handleSearch} @keydown=${this.handleSearchKeydown} />
            <button type="button" class="sv-search-clear ${isSearching ? "" : "is-hidden"}" title="Clear search (Esc)" aria-label="Clear search" @click=${this.clearSearch}>✕</button>
          </div>

          <div class="sv-tab ${this.activeTab === "transcription" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('transcription')}>🗣️ Transcription</div>
          <div class="sv-tab ${this.activeTab === "capture" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('capture')}>🎙️ Capture</div>
          <div class="sv-tab ${this.activeTab === "appearance" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('appearance')}>🎨 Appearance</div>
          <div class="sv-tab ${this.activeTab === "tags" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('tags')}>🏷️ Tags</div>
          <div class="sv-tab ${this.activeTab === "postprocessing" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('postprocessing')}>✨ Post-Processing</div>
          <div class="sv-tab ${this.activeTab === "system" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('system')}>⚙️ System</div>
          
          ${isSearching ? html`<div class="sv-tab active" style="margin-top: 12px; font-style: italic;">Search Results</div>` : ""}
        </div>
        <div class="settings-main">
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

// Legacy wrapper
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

  dispose() {
    this.element.remove();
  }
}
