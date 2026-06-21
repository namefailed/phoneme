import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import {
  listTags,
  attachTag,
  type Recording,
  type Tag,
} from "../../services/ipc";
import { showToast } from "../../utils/toast";

/** Injected by RecordingsView: re-query the list after a bulk mutation, and
 *  clear the multi-selection (which unmounts the bar). */
export type BulkActionCallbacks = {
  onRefresh: () => void;
  onClear: () => void;
};

type ExportFormat = "txt" | "json" | "csv";

/** Persisted floating position of the bar (drag it by the ⠿ handle);
 *  Ctrl+Shift+click the handle resets to the default bottom-center. */
const POS_LS = "phoneme.bulkBarPos";

/**
 * The floating bar that appears while recordings are multi-selected (Space /
 * Shift+↑↓ in the list). Buttons: Re-run… (the shared RerunForm in a modal,
 * applied to every selected id), Tag (attach one tag to all), Export
 * (txt/json/csv via a save dialog), Side-by-side (exactly two selected —
 * dispatches `phoneme:open-split`), Delete (`phoneme:request-delete`, the
 * undoable flow), and ✕ Clear.
 *
 * State: the list owns the selection itself — this gets it as the `selected`
 * property on every change; its own state is just menu/busy/drag bookkeeping.
 * Keyboard: Shift+Enter in the list dispatches
 * `phoneme:enter-bulk-bar`, which starts a roving h/l cursor over the
 * buttons (Enter activates, j/k/Esc return to the list); its Escape handler
 * runs capture-phase so an open menu closes without clearing the selection.
 * Mounted/unmounted by RecordingsView per selection change.
 */
@customElement('ph-bulk-action-bar')
export class BulkActionBarElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM to inherit global .bulk-bar styling
  }

  @property({ type: Object }) selected: ReadonlySet<string> = new Set();
  @property({ type: Array }) recordings: ReadonlyArray<Recording> = [];
  @property({ type: Object }) callbacks!: BulkActionCallbacks;

  @state() private busy = false;
  /** Floating position; null = default (bottom-center). Persisted per device. */
  @state() private pos: { x: number; y: number } | null = null;
  @state() private allTags: Tag[] = [];
  @state() private openMenu: "tag" | "export" | null = null;

  private docClick = (e: MouseEvent) => {
    // Close an open dropdown when clicking outside the bar.
    if (this.openMenu && !e.composedPath().some((n) => (n as Element)?.classList?.contains?.("bulk-bar"))) {
      this.openMenu = null;
    }
  };
  /** Escape closes the open menu/modal (rerun · tag · export) — capture-phase +
   *  stopPropagation so it never reaches the list (which would clear the
   *  selection) or close the recording. */
  private onEsc = (e: KeyboardEvent) => {
    // Only handle the mouse-opened case here; when the keyboard nav owns the bar
    // (navIndex >= 0), onBulkNavKey closes the menu AND keeps the cursor on the
    // opener button instead of falling through to "exit the bar".
    if (e.key === "Escape" && this.openMenu && this.navIndex < 0) {
      e.preventDefault();
      e.stopPropagation();
      this.openMenu = null;
    }
  };

  /** Roving keyboard cursor over the bar's buttons (Shift+Enter from the list
   *  enters this); -1 = not active. */
  @state() private navIndex = -1;
  /** Cursor within an open Tag/Export dropdown's items (j/k cycle, Enter picks). */
  @state() private menuIndex = 0;

  private bulkButtons(): HTMLElement[] {
    return [...this.querySelectorAll<HTMLElement>(".bulk-bar .bulk-btn")].filter((b) => b.offsetParent !== null);
  }

  private highlightBulkNav() {
    this.querySelectorAll(".bulk-btn.kbd-cursor").forEach((b) => b.classList.remove("kbd-cursor"));
    this.bulkButtons()[this.navIndex]?.classList.add("kbd-cursor");
  }

  /** The clickable items inside the currently-open Tag/Export dropdown. */
  private menuItems(): HTMLElement[] {
    return [...this.querySelectorAll<HTMLElement>(".bulk-menu .bulk-menu-item")].filter((b) => b.offsetParent !== null);
  }

  private highlightMenu() {
    this.querySelectorAll(".bulk-menu-item.kbd-cursor").forEach((b) => b.classList.remove("kbd-cursor"));
    const items = this.menuItems();
    if (!items.length) return;
    if (this.menuIndex >= items.length) this.menuIndex = items.length - 1;
    if (this.menuIndex < 0) this.menuIndex = 0;
    items[this.menuIndex].classList.add("kbd-cursor");
    items[this.menuIndex].scrollIntoView({ block: "nearest" });
  }

  /** Shift+Enter from the list hands keyboard control to the bar. */
  private onEnterBulkBar = () => {
    if (this.selected.size === 0) return;
    this.navIndex = 0;
    void this.updateComplete.then(() => this.highlightBulkNav());
  };

  private exitBulkNav() {
    this.navIndex = -1;
    this.querySelectorAll(".bulk-btn.kbd-cursor").forEach((b) => b.classList.remove("kbd-cursor"));
    // Back to the recordings list, on the last-selected file.
    window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "focus-list" } }));
  }

  /** Keyboard nav inside the bar: h/l move, Enter activates, j/k/Esc exit.
   *  Capture-phase + stopPropagation so it beats the list/global handlers; only
   *  active once Shift+Enter entered the nav and no dropdown/modal owns keys. */
  private onBulkNavKey = (e: KeyboardEvent) => {
    // `\` with exactly two recordings selected (and not typing) opens them side
    // by side — same action as the bar's "Side by side" button.
    if (e.key === "\\" && this.selected.size === 2) {
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
      if (!typing) {
        e.preventDefault();
        e.stopPropagation();
        void this.openSideBySide();
      }
      return;
    }
    // Shift+Enter (while a selection exists, and not typing in a field) hands
    // control to the bar. The bar is only mounted when something is selected, so
    // this listener is dormant otherwise.
    if (e.key === "Enter" && e.shiftKey && this.navIndex < 0 && !this.openMenu) {
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
      if (!typing && this.selected.size > 0) {
        e.preventDefault();
        e.stopPropagation();
        this.onEnterBulkBar();
      }
      return;
    }
    // A Tag/Export dropdown is open and the keyboard owns the bar: drive its
    // items with j/k, pick with Enter, close with Esc/h/l (back to the opener).
    if (this.openMenu && this.navIndex >= 0) {
      const items = this.menuItems();
      switch (e.key) {
        case "j": case "ArrowDown":
          e.preventDefault(); e.stopPropagation();
          if (items.length) { this.menuIndex = (this.menuIndex + 1) % items.length; this.highlightMenu(); }
          return;
        case "k": case "ArrowUp":
          e.preventDefault(); e.stopPropagation();
          if (items.length) { this.menuIndex = (this.menuIndex - 1 + items.length) % items.length; this.highlightMenu(); }
          return;
        case "Enter": case " ":
          e.preventDefault(); e.stopPropagation();
          items[this.menuIndex]?.click(); // the item's own handler closes the menu
          return;
        case "Escape": case "h": case "l":
          e.preventDefault(); e.stopPropagation();
          this.openMenu = null;
          void this.updateComplete.then(() => this.highlightBulkNav());
          return;
      }
      // Swallow anything else so it can't leak to the list while a menu is open.
      e.preventDefault(); e.stopPropagation();
      return;
    }
    if (this.navIndex < 0) return;
    const btns = this.bulkButtons();
    if (!btns.length) return;
    switch (e.key) {
      case "h": case "ArrowLeft":
        e.preventDefault(); e.stopPropagation();
        this.navIndex = Math.max(0, this.navIndex - 1); this.highlightBulkNav(); return;
      case "l": case "ArrowRight":
        e.preventDefault(); e.stopPropagation();
        this.navIndex = Math.min(btns.length - 1, this.navIndex + 1); this.highlightBulkNav(); return;
      case "Enter": case " ":
        e.preventDefault(); e.stopPropagation();
        btns[this.navIndex]?.click();
        // After the action settles (one frame, so a modal/menu has mounted): if it
        // opened an in-bar dropdown (Tag/Export) or a modal (Re-run), those own the
        // keyboard next, so leave the cursor be. Otherwise the action closed the bar
        // (Delete/Deselect clear the selection and unmount it), so hand the cursor
        // and its glow back to the list instead of stranding it on the removed
        // button.
        requestAnimationFrame(() => {
          const modalOpen = !!document.querySelector('[class*="modal-overlay"]');
          if (this.navIndex >= 0 && !this.openMenu && !modalOpen) this.exitBulkNav();
        });
        return;
      case "j": case "k": case "ArrowDown": case "ArrowUp": case "Escape":
        e.preventDefault(); e.stopPropagation();
        this.exitBulkNav(); return;
    }
  };

  connectedCallback() {
    super.connectedCallback();
    try {
      const raw = localStorage.getItem(POS_LS);
      if (raw) {
        const p = JSON.parse(raw);
        // Only honour a saved drag position that's still on-screen — a stale
        // off-screen position (window was resized smaller, or it was dragged
        // out) would mount the whole bar where it can't be seen ("bar gone").
        if (
          typeof p?.x === "number" && typeof p?.y === "number" &&
          p.x >= 0 && p.x <= window.innerWidth - 80 &&
          p.y >= 0 && p.y <= window.innerHeight - 40
        ) {
          this.pos = p;
        } else {
          localStorage.removeItem(POS_LS);
        }
      }
    } catch { /* ignore */ }
    void this.loadTags();
    document.addEventListener("click", this.docClick);
    document.addEventListener("keydown", this.onEsc, true);
    document.addEventListener("keydown", this.onBulkNavKey, true);
    window.addEventListener("phoneme:enter-bulk-bar", this.onEnterBulkBar);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("click", this.docClick);
    document.removeEventListener("keydown", this.onEsc, true);
    document.removeEventListener("keydown", this.onBulkNavKey, true);
    window.removeEventListener("phoneme:enter-bulk-bar", this.onEnterBulkBar);
  }

  private async loadTags() {
    try {
      this.allTags = await listTags();
    } catch {
      this.allTags = [];
    }
  }

  private selectedRecordings(): Recording[] {
    return this.recordings.filter((r) => this.selected.has(r.id));
  }

  updated(changed: PropertyValues) {
    if (changed.has('selected')) {
      const display = this.selected.size > 0 ? "block" : "none";
      if (this.style.display !== display) this.style.display = display;
    }
  }

  // ── Drag ────────────────────────────────────────────────────────────────
  private startDrag(e: MouseEvent) {
    e.preventDefault();
    // Ctrl+Shift-click resets to the default bottom-centre position.
    if (e.ctrlKey && e.shiftKey) {
      e.stopPropagation();
      this.pos = null;
      try { localStorage.removeItem(POS_LS); } catch { /* ignore */ }
      return;
    }
    const startX = e.clientX;
    const startY = e.clientY;
    const bar = (e.currentTarget as HTMLElement).closest<HTMLElement>(".bulk-bar");
    const rect = bar?.getBoundingClientRect();
    const base = this.pos ?? { x: rect?.left ?? 0, y: rect?.top ?? 0 };
    const onMove = (m: MouseEvent) => {
      this.pos = {
        x: Math.max(8, Math.min(window.innerWidth - 120, base.x + (m.clientX - startX))),
        y: Math.max(8, Math.min(window.innerHeight - 48, base.y + (m.clientY - startY))),
      };
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      if (this.pos) {
        try { localStorage.setItem(POS_LS, JSON.stringify(this.pos)); } catch { /* ignore */ }
      }
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }

  // ── Bulk operations over the selection ────────────────────────────────────
  /** Run `op` over every selected recording, report a combined toast, refresh. */
  private async runOverSelection(op: (r: Recording) => Promise<void>, verb: string, clear = true) {
    if (this.busy) return;
    this.openMenu = null;
    this.busy = true;
    const recs = this.selectedRecordings();
    let ok = 0;
    let failed = 0;
    for (const r of recs) {
      try { await op(r); ok++; } catch { failed++; }
    }
    this.busy = false;
    if (failed === 0) showToast(`${verb} ${ok} recording${ok !== 1 ? "s" : ""}.`, "success");
    else showToast(`${ok} ok, ${failed} failed.`, "error");
    if (clear) this.callbacks.onClear();
    this.callbacks.onRefresh();
  }

  /** Open the two selected recordings in split mode (two full panes). Only
   *  meaningful with exactly two selected — the button only shows then, and the
   *  `\` shortcut is gated the same way. RecordingsView owns the panes. */
  private async openSideBySide() {
    if (this.selected.size !== 2) return;
    const [a, b] = [...this.selected];
    window.dispatchEvent(new CustomEvent("phoneme:open-split", { detail: { a, b } }));
  }

  /** Open the unified Models modal in one-shot mode for the whole selection —
   *  the same context-aware modal the detail Re-run and the header Quick
   *  Switcher use, so bulk and single Re-run are identical. "Run once" there
   *  re-runs each selected recording's whole pipeline; "Save as default"
   *  persists the chosen models. */
  private async openBulkRerun() {
    if (this.busy) return;
    const ids = [...this.selected];
    if (!ids.length) return;
    this.openMenu = null;
    const { openModelPicker } = await import("../ModelPicker");
    await openModelPicker("transcription", undefined, { mode: "oneshot", recordingIds: ids });
    // The daemon emits queue events that refresh the list as each re-run starts.
    this.callbacks.onRefresh();
  }

  private async handleTag(tag: Tag) {
    await this.runOverSelection((r) => attachTag(r.id, tag.id), `Tagged "${tag.name}" on`, false);
  }

  private buildExport(format: ExportFormat): { data: string; mime: string; ext: string } | null {
    const recs = this.selectedRecordings().filter((r) => r.transcript);
    if (recs.length === 0) return null;
    if (format === "json") {
      const arr = recs.map((r) => ({
        id: r.id,
        started_at: r.started_at,
        duration_ms: r.duration_ms,
        model: r.model ?? null,
        transcript: r.transcript ?? "",
      }));
      return { data: JSON.stringify(arr, null, 2), mime: "application/json", ext: "json" };
    }
    if (format === "csv") {
      const esc = (s: string) => `"${String(s).replace(/"/g, '""')}"`;
      const rows = [["id", "started_at", "duration_ms", "model", "transcript"].join(",")];
      for (const r of recs) {
        rows.push([r.id, r.started_at, String(r.duration_ms), r.model ?? "", esc(r.transcript ?? "")].join(","));
      }
      return { data: rows.join("\n"), mime: "text/csv", ext: "csv" };
    }
    // txt
    const lines: string[] = [];
    for (const r of recs) {
      lines.push(`=== ${new Date(r.started_at).toLocaleString()} (${r.id}) ===`);
      lines.push((r.transcript ?? "").trim());
      lines.push("");
    }
    return { data: lines.join("\n"), mime: "text/plain", ext: "txt" };
  }

  private handleExport(format: ExportFormat) {
    this.openMenu = null;
    const built = this.buildExport(format);
    if (!built) {
      showToast("No transcripts to export in the selection.", "error");
      return;
    }
    const blob = new Blob([built.data], { type: built.mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `phoneme-export-${Date.now()}.${built.ext}`;
    a.click();
    URL.revokeObjectURL(url);
    showToast(`Exported ${this.selectedRecordings().filter((r) => r.transcript).length} transcript(s) as ${format.toUpperCase()}.`, "success");
  }

  private handleDelete() {
    if (this.busy) return;
    this.openMenu = null;
    const ids = [...this.selected];
    if (!ids.length) return;
    // RecordingsView runs the grace-period Undo flow (hides the rows now, only
    // deletes for real when the Undo toast lapses) and clears this selection.
    window.dispatchEvent(new CustomEvent("phoneme:request-delete", { detail: { ids } }));
  }

  private toggleMenu(menu: "tag" | "export", e: Event) {
    e.stopPropagation();
    if (this.openMenu === menu) {
      this.openMenu = null;
      return;
    }
    this.openMenu = menu;
    this.menuIndex = 0;
    // If the bar is being keyboard-driven (Enter opened this), highlight the
    // first item once it renders so j/k pick up from there.
    if (this.navIndex >= 0) void this.updateComplete.then(() => this.highlightMenu());
  }

  render() {
    const n = this.selected.size;
    if (n === 0) return html``;

    const style = this.pos
      ? `position:fixed; left:${Math.max(8, Math.min(window.innerWidth - 80, this.pos.x))}px; top:${Math.max(8, Math.min(window.innerHeight - 40, this.pos.y))}px;`
      : `position:fixed; left:50%; bottom:24px; transform:translateX(-50%);`;

    return html`
      <div class="bulk-bar" style=${style}>
        <span class="bulk-grip" title="Drag to move · Ctrl+Shift-click to reset" @mousedown=${(e: MouseEvent) => this.startDrag(e)}>⠿</span>
        <span class="bulk-count">${this.busy ? "Working…" : `${n} selected`}</span>
        <div class="bulk-actions">
          <button class="bulk-btn" title="Re-run the selected recordings with chosen models" .disabled=${this.busy} @click=${this.openBulkRerun}>↻ Re-run</button>

          ${n === 2 ? html`<button class="bulk-btn" title="View the two selected recordings side by side (\\)" .disabled=${this.busy} @click=${this.openSideBySide}>◫ Side by side</button>` : null}

          <span class="bulk-menu-wrap">
            <button class="bulk-btn" title="Add a tag to selected" .disabled=${this.busy} @click=${(e: Event) => this.toggleMenu("tag", e)}>🏷️ Tag <svg class="ph-caret-ico ${this.openMenu === "tag" ? "open" : ""}" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>
            ${this.openMenu === "tag" ? html`
              <div class="bulk-menu" @click=${(e: Event) => e.stopPropagation()}>
                ${this.allTags.length === 0
                  ? html`<div class="bulk-menu-empty">No tags yet — create some from a recording's detail view.</div>`
                  : this.allTags.map((t) => html`
                    <button class="bulk-menu-item" @click=${() => this.handleTag(t)}>
                      <span class="bulk-menu-dot" style="background:${t.color || 'var(--accent)'}"></span>${t.name}
                    </button>`)}
              </div>` : null}
          </span>

          <span class="bulk-menu-wrap">
            <button class="bulk-btn" title="Export transcripts" .disabled=${this.busy} @click=${(e: Event) => this.toggleMenu("export", e)}>↓ Export <svg class="ph-caret-ico ${this.openMenu === "export" ? "open" : ""}" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>
            ${this.openMenu === "export" ? html`
              <div class="bulk-menu" @click=${(e: Event) => e.stopPropagation()}>
                <button class="bulk-menu-item" @click=${() => this.handleExport("txt")}>Plain text (.txt)</button>
                <button class="bulk-menu-item" @click=${() => this.handleExport("json")}>JSON (.json)</button>
                <button class="bulk-menu-item" @click=${() => this.handleExport("csv")}>CSV (.csv)</button>
              </div>` : null}
          </span>

          <button class="bulk-btn bulk-btn--danger" title="Delete selected" .disabled=${this.busy} @click=${this.handleDelete}>🗑 Delete</button>
          <button class="bulk-btn bulk-btn--muted" title="Deselect all" .disabled=${this.busy} @click=${() => this.callbacks.onClear()}>✕ Deselect</button>
        </div>
      </div>
    `;
  }
}

/** Imperative mount wrapper used by RecordingsView: mounts the bar with the
 *  current selection and pushes later selection changes via `update`. */
export class BulkActionBar {
  private element: BulkActionBarElement;
  constructor(
    container: HTMLElement,
    selected: ReadonlySet<string>,
    recordings: ReadonlyArray<Recording>,
    callbacks: BulkActionCallbacks,
  ) {
    this.element = document.createElement('ph-bulk-action-bar') as BulkActionBarElement;
    this.element.selected = selected;
    this.element.recordings = recordings;
    this.element.callbacks = callbacks;
    container.appendChild(this.element);
  }

  update(selected: ReadonlySet<string>, recordings: ReadonlyArray<Recording>) {
    this.element.selected = selected;
    this.element.recordings = recordings;
  }
}
