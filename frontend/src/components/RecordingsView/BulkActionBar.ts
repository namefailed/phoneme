import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import {
  deleteRecording,
  retranscribeRecording,
  rerunCleanup,
  rerunSummary,
  listTags,
  attachTag,
  type Recording,
  type Tag,
} from "../../services/ipc";
import { showToast } from "../../utils/toast";

export type BulkActionCallbacks = {
  onRefresh: () => void;
  onClear: () => void;
};

type ExportFormat = "txt" | "json" | "csv";

const POS_LS = "phoneme.bulkBarPos";

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
  @state() private openMenu: "rerun" | "tag" | "export" | null = null;

  private docClick = (e: MouseEvent) => {
    // Close an open dropdown when clicking outside the bar.
    if (this.openMenu && !e.composedPath().some((n) => (n as Element)?.classList?.contains?.("bulk-bar"))) {
      this.openMenu = null;
    }
  };

  connectedCallback() {
    super.connectedCallback();
    try {
      const raw = localStorage.getItem(POS_LS);
      if (raw) {
        const p = JSON.parse(raw);
        if (typeof p?.x === "number" && typeof p?.y === "number") this.pos = p;
      }
    } catch { /* ignore */ }
    void this.loadTags();
    document.addEventListener("click", this.docClick);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("click", this.docClick);
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

  private handleRetranscribe() {
    void this.runOverSelection((r) => retranscribeRecording(r.id), "Queued for re-transcription:");
  }

  private handleCleanup() {
    void this.runOverSelection((r) => rerunCleanup(r.id), "Re-running cleanup on", false);
  }

  private handleSummarize() {
    void this.runOverSelection((r) => rerunSummary(r.id), "Summarizing", false);
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

  private async handleDelete() {
    if (this.busy) return;
    this.openMenu = null;
    const n = this.selected.size;
    const { confirmDelete } = await import("../ConfirmDelete");
    const confirmed = await confirmDelete({
      title: n === 1 ? "Delete Recording?" : `Delete ${n} Recordings?`,
      body: n === 1
        ? "This will permanently delete the recording and its audio file. This action cannot be undone."
        : `This will permanently delete ${n} recordings and their audio files. This action cannot be undone.`,
      confirmLabel: n === 1 ? "Delete" : `Delete ${n} Recordings`,
      skipKey: "phoneme_skip_bulk_delete_confirm",
    });
    if (!confirmed) return;
    await this.runOverSelection((r) => deleteRecording(r.id, false), "Deleted");
  }

  private toggleMenu(menu: "rerun" | "tag" | "export", e: Event) {
    e.stopPropagation();
    this.openMenu = this.openMenu === menu ? null : menu;
  }

  render() {
    const n = this.selected.size;
    if (n === 0) return html``;

    const style = this.pos
      ? `position:fixed; left:${this.pos.x}px; top:${this.pos.y}px;`
      : `position:fixed; left:50%; bottom:24px; transform:translateX(-50%);`;

    return html`
      <div class="bulk-bar" style=${style}>
        <span class="bulk-grip" title="Drag to move" @mousedown=${(e: MouseEvent) => this.startDrag(e)}>⠿</span>
        <span class="bulk-count">${this.busy ? "Working…" : `${n} selected`}</span>
        <div class="bulk-actions">
          <span class="bulk-menu-wrap">
            <button class="bulk-btn" title="Re-run a step on the selected recordings" .disabled=${this.busy} @click=${(e: Event) => this.toggleMenu("rerun", e)}>↻ Re-run ▾</button>
            ${this.openMenu === "rerun" ? html`
              <div class="bulk-menu" @click=${(e: Event) => e.stopPropagation()}>
                <button class="bulk-menu-item" @click=${this.handleRetranscribe}>↺ Re-transcribe (everything)</button>
                <button class="bulk-menu-item" @click=${this.handleCleanup}>✨ Re-run cleanup</button>
                <button class="bulk-menu-item" @click=${this.handleSummarize}>📝 Re-summarize</button>
              </div>` : null}
          </span>

          <span class="bulk-menu-wrap">
            <button class="bulk-btn" title="Add a tag to selected" .disabled=${this.busy} @click=${(e: Event) => this.toggleMenu("tag", e)}>🏷 Tag ▾</button>
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
            <button class="bulk-btn" title="Export transcripts" .disabled=${this.busy} @click=${(e: Event) => this.toggleMenu("export", e)}>↓ Export ▾</button>
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

// Vanilla wrapper used by RecordingsView.
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
