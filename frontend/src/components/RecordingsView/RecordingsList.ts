import { listRecordings, type Recording } from "../../services/ipc";
import { Store } from "../../state/store";
import { filterStore } from "../../state/filter";
import { invoke } from "@tauri-apps/api/core";
import { formatDay } from "../../utils/date";
import {
  formatDuration,
  formatTime,
  statusToClass,
  statusLabel,
  highlightMatch,
} from "../../utils/format";
import "../shared/styles.css";
import "./styles.css";

export type RecordingsListState = {
  recordings: Recording[];
  selectedId: string | null;
  loading: boolean;
  error: string | null;
};

export class RecordingsList {
  private container: HTMLElement;
  private state: Store<RecordingsListState>;
  private onSelect: (id: string) => void;
  /** Notify parent that the multi-selection set has changed. */
  private onSelectionChange: (ids: Set<string>) => void;
  /** Cached app config — loaded once and refreshed when settings are saved. */
  private config: any = null;
  private currentWidths: string[] | null = null;
  /** Index of the keyboard-focused row (-1 = none). */
  private focusedIndex = -1;

  // ── Multi-selection state ────────────────────────────────────────────────
  /** IDs that are currently multi-selected (checkboxes). */
  private multiSelected = new Set<string>();
  /** Index of the anchor row for Shift+Click range selection. */
  private anchorIndex = -1;

  constructor(
    container: HTMLElement,
    state: Store<RecordingsListState>,
    onSelect: (id: string) => void,
    onSelectionChange: (ids: Set<string>) => void = () => {},
  ) {
    this.container = container;
    this.state = state;
    this.onSelect = onSelect;
    this.onSelectionChange = onSelectionChange;
    state.subscribe(() => this.render());
    filterStore.subscribe(() => { void this.refresh(); });

    // Reload config whenever settings are saved so column visibility stays fresh.
    window.addEventListener("config:saved", (e) => {
      this.config = (e as CustomEvent).detail ?? null;
    });
  }

  async refresh() {
    this.state.set({ ...this.state.get(), loading: true, error: null });
    try {
      const f = filterStore.get();
      // Config is lazy-loaded once and then updated via the config:saved event.
      if (!this.config) {
        this.config = await invoke("read_config");
      }
      const rows = await listRecordings({ ...f, limit: 200 });
      // Prune any multi-selected ids that are no longer in the list.
      const ids = new Set(rows.map((r) => r.id));
      this.multiSelected.forEach((id) => {
        if (!ids.has(id)) this.multiSelected.delete(id);
      });
      this.state.set({ ...this.state.get(), recordings: rows, loading: false });
    } catch (e) {
      this.state.set({ ...this.state.get(), error: String(e), loading: false });
    }
  }

  /** Clear all multi-selection state and re-render. */
  clearSelection() {
    this.multiSelected.clear();
    this.anchorIndex = -1;
    this.onSelectionChange(new Set());
    this.render();
  }

  /** Select all currently-loaded recordings. */
  selectAll() {
    const recs = this.state.get().recordings;
    recs.forEach((r) => this.multiSelected.add(r.id));
    this.onSelectionChange(new Set(this.multiSelected));
    this.render();
  }

  getMultiSelected(): Set<string> {
    return new Set(this.multiSelected);
  }

  render() {
    const s = this.state.get();
    if (s.loading && s.recordings.length === 0) {
      this.container.innerHTML = `<div class="empty">Loading…</div>`;
      return;
    }
    if (s.error) {
      this.container.innerHTML = `<div class="empty error">${s.error}</div>`;
      return;
    }
    if (s.recordings.length === 0) {
      this.container.innerHTML = `<div class="empty">
        <h3 style="margin-bottom: 8px; color: var(--fg-default);">No recordings found</h3>
        <p style="color: var(--fg-muted); margin-bottom: 12px;">Press your global hotkey to start speaking, or click the Record button in the top right.</p>
        <p class="hint" style="font-size: 11px;">You can also use the CLI: <code>phoneme record --oneshot</code></p>
      </div>`;
      return;
    }

    const visibleCols: string[] = (this.config as any)?.interface?.visible_columns || [
      "time",
      "duration",
      "status",
      "transcript",
    ];
    this.currentWidths = (this.config as any)?.interface?.column_widths || null;

    const colLabels: Record<string, string> = {
      day: "Day",
      time: "Time",
      duration: "Dur",
      status: "Status",
      transcript: "Transcript",
    };

    const colWidths: Record<string, string> = {
      day: "85px",
      time: "94px",
      duration: "84px",
      status: "89px",
      transcript: "1fr",
    };

    if (!this.currentWidths || this.currentWidths.length !== visibleCols.length) {
      this.currentWidths = visibleCols.map((c) => colWidths[c] || "auto");
    }

    // Prepend the checkbox column to the grid.
    const checkboxColWidth = "28px";
    const gridTemplate = [checkboxColWidth, ...this.currentWidths].join(" ");

    // Header: checkbox (select-all) + data columns.
    const allSelected =
      s.recordings.length > 0 &&
      s.recordings.every((r) => this.multiSelected.has(r.id));
    const someSelected = this.multiSelected.size > 0 && !allSelected;

    const headSpans = visibleCols
      .map(
        (c, i) => `
      <span class="col-head" data-col="${i + 1}">
        ${colLabels[c] || c}
        ${i < visibleCols.length - 1 ? `<div class="resizer" data-col="${i + 1}"></div>` : ""}
      </span>
    `,
      )
      .join("");

    const head = `
      <div class="rec-table-head" style="grid-template-columns: ${gridTemplate}">
        <span class="col-head col-checkbox">
          <input
            type="checkbox"
            id="select-all-cb"
            class="row-cb"
            ${allSelected ? "checked" : ""}
            ${someSelected ? "data-indeterminate" : ""}
            title="${allSelected ? "Deselect all" : "Select all"}"
            aria-label="${allSelected ? "Deselect all" : "Select all"}"
          />
        </span>
        ${headSpans}
      </div>
    `;

    // Clamp focusedIndex to valid range after list changes
    if (this.focusedIndex >= s.recordings.length) {
      this.focusedIndex = s.recordings.length - 1;
    }

    const rows = s.recordings
      .map((r, i) =>
        this.renderRow(
          r,
          r.id === s.selectedId,
          i === this.focusedIndex,
          this.multiSelected.has(r.id),
          visibleCols,
          gridTemplate,
        ),
      )
      .join("");

    this.container.innerHTML = `<div class="rec-table" tabindex="0" role="listbox" aria-label="Recordings">${head}${rows}</div>`;

    // Restore indeterminate state on the select-all checkbox (can't be set via HTML attr).
    const selectAllCb = this.container.querySelector<HTMLInputElement>("#select-all-cb");
    if (selectAllCb && someSelected) selectAllCb.indeterminate = true;

    const table = this.container.querySelector<HTMLElement>(".rec-table")!;

    // ── Keyboard navigation ───────────────────────────────────────────────────
    table.addEventListener("keydown", (e: KeyboardEvent) => {
      const recs = this.state.get().recordings;
      if (!recs.length) return;

      // Ctrl+A = select all
      if (e.ctrlKey && e.key === "a") {
        e.preventDefault();
        this.selectAll();
        return;
      }
      // Escape = clear selection
      if (e.key === "Escape" && this.multiSelected.size > 0) {
        e.preventDefault();
        this.clearSelection();
        return;
      }

      if (e.key === "ArrowDown") {
        e.preventDefault();
        const next = Math.min(this.focusedIndex + 1, recs.length - 1);
        if (e.shiftKey) {
          this.toggleRangeSelect(next, recs);
        }
        this.moveFocus(next);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        const prev = Math.max(this.focusedIndex - 1, 0);
        if (e.shiftKey) {
          this.toggleRangeSelect(prev, recs);
        }
        this.moveFocus(prev);
      } else if (e.key === "Enter" && this.focusedIndex >= 0) {
        e.preventDefault();
        const id = recs[this.focusedIndex]?.id;
        if (id) this.onSelect(id);
      } else if (e.key === " " && this.focusedIndex >= 0) {
        // Space = toggle checkbox of focused row
        e.preventDefault();
        const id = recs[this.focusedIndex]?.id;
        if (id) this.toggleId(id, this.focusedIndex);
      }
    });

    // ── Row click ──────────────────────────────────────────────────────────
    this.container.querySelectorAll<HTMLElement>(".rec-row").forEach((el, i) => {
      el.addEventListener("click", (e: MouseEvent) => {
        const id = el.getAttribute("data-id");
        if (!id) return;

        // Click on the checkbox cell → toggle selection only, don't navigate.
        const target = e.target as HTMLElement;
        if (target.classList.contains("row-cb") || target.closest(".col-checkbox")) {
          // The checkbox input already reflects the new state via its change event;
          // we handle toggle here directly so Shift+Click range works too.
          if (e.shiftKey && this.anchorIndex >= 0) {
            this.selectRange(this.anchorIndex, i, this.state.get().recordings);
          } else {
            this.toggleId(id, i);
          }
          return;
        }

        // Normal row click: Shift = range select; otherwise single-select navigate.
        if (e.shiftKey && this.anchorIndex >= 0) {
          this.selectRange(this.anchorIndex, i, this.state.get().recordings);
          return;
        }

        this.focusedIndex = i;
        this.updateFocusClasses();
        this.onSelect(id);
        table.focus({ preventScroll: true });
      });
    });

    // ── Checkbox change events ─────────────────────────────────────────────
    // Delegate: one listener on the table catches all checkbox change events.
    table.addEventListener("change", (e: Event) => {
      const cb = e.target as HTMLInputElement;
      if (!cb || cb.type !== "checkbox") return;
      if (cb.id === "select-all-cb") {
        if (cb.checked) {
          this.selectAll();
        } else {
          this.clearSelection();
        }
        return;
      }
      // Individual row checkbox — id is encoded in data-id of the parent row.
      const row = cb.closest<HTMLElement>(".rec-row");
      const id = row?.getAttribute("data-id");
      if (!id) return;
      const idx = this.state.get().recordings.findIndex((r) => r.id === id);
      if (cb.checked) {
        this.multiSelected.add(id);
        this.anchorIndex = idx;
      } else {
        this.multiSelected.delete(id);
      }
      this.onSelectionChange(new Set(this.multiSelected));
      this.updateCheckboxesOnly();
    });

    // ── Column resizing ────────────────────────────────────────────────────
    this.container.querySelectorAll<HTMLElement>(".resizer").forEach((el) => {
      el.addEventListener("mousedown", (e: MouseEvent) => {
        e.preventDefault();
        e.stopPropagation();

        // Column indices in the grid are shifted by 1 because of the checkbox col.
        const colIdx = parseInt(el.getAttribute("data-col") || "0", 10) - 1;
        const startX = e.clientX;
        const heads = Array.from(this.container.querySelectorAll(".col-head")).slice(1); // skip checkbox col
        const startW = (heads[colIdx] as HTMLElement).offsetWidth;

        const onMove = (moveEvent: MouseEvent) => {
          if (!this.currentWidths) return;
          const newW = Math.max(30, startW + moveEvent.clientX - startX);
          this.currentWidths[colIdx] = `${newW}px`;
          const template = [checkboxColWidth, ...this.currentWidths].join(" ");
          const headRow = this.container.querySelector<HTMLElement>(".rec-table-head");
          if (headRow) headRow.style.gridTemplateColumns = template;
          this.container.querySelectorAll<HTMLElement>(".rec-row").forEach((row) => {
            row.style.gridTemplateColumns = template;
          });
        };

        const onUp = () => {
          document.removeEventListener("mousemove", onMove);
          document.removeEventListener("mouseup", onUp);
          if (this.config && this.currentWidths) {
            (this.config as any).interface = (this.config as any).interface || {};
            (this.config as any).interface.column_widths = this.currentWidths;
            invoke("write_config", { config: this.config }).catch(console.error);
          }
        };

        document.addEventListener("mousemove", onMove);
        document.addEventListener("mouseup", onUp);
      });
    });
  }

  // ── Selection helpers ──────────────────────────────────────────────────────

  private toggleId(id: string, index: number) {
    if (this.multiSelected.has(id)) {
      this.multiSelected.delete(id);
    } else {
      this.multiSelected.add(id);
      this.anchorIndex = index;
    }
    this.onSelectionChange(new Set(this.multiSelected));
    this.updateCheckboxesOnly();
  }

  private selectRange(from: number, to: number, recs: Recording[]) {
    const [lo, hi] = from < to ? [from, to] : [to, from];
    for (let i = lo; i <= hi; i++) {
      if (recs[i]) this.multiSelected.add(recs[i].id);
    }
    this.anchorIndex = to;
    this.onSelectionChange(new Set(this.multiSelected));
    this.updateCheckboxesOnly();
  }

  private toggleRangeSelect(newIndex: number, recs: Recording[]) {
    // Shift+Arrow: extend range from current focusedIndex to newIndex.
    if (this.anchorIndex < 0) this.anchorIndex = this.focusedIndex;
    this.selectRange(this.anchorIndex, newIndex, recs);
  }

  /** Update only checkbox states without a full re-render (cheaper). */
  private updateCheckboxesOnly() {
    const recs = this.state.get().recordings;
    const allSelected =
      recs.length > 0 && recs.every((r) => this.multiSelected.has(r.id));
    const someSelected = this.multiSelected.size > 0 && !allSelected;

    const selectAllCb = this.container.querySelector<HTMLInputElement>("#select-all-cb");
    if (selectAllCb) {
      selectAllCb.checked = allSelected;
      selectAllCb.indeterminate = someSelected;
    }

    this.container.querySelectorAll<HTMLElement>(".rec-row").forEach((el) => {
      const id = el.getAttribute("data-id");
      if (!id) return;
      const cb = el.querySelector<HTMLInputElement>(".row-cb");
      const checked = this.multiSelected.has(id);
      if (cb) cb.checked = checked;
      el.classList.toggle("multi-selected", checked);
    });
  }

  private moveFocus(newIndex: number) {
    this.focusedIndex = newIndex;
    this.updateFocusClasses();
    // Scroll focused row into view
    const rows = this.container.querySelectorAll<HTMLElement>(".rec-row");
    rows[this.focusedIndex]?.scrollIntoView({ block: "nearest" });
  }

  private updateFocusClasses() {
    this.container.querySelectorAll<HTMLElement>(".rec-row").forEach((el, i) => {
      el.classList.toggle("kbd-focused", i === this.focusedIndex);
    });
  }

  private renderRow(
    r: Recording,
    active: boolean,
    kbFocused: boolean,
    multiChecked: boolean,
    visibleCols: string[],
    gridTemplate: string,
  ): string {
    const day = formatDay(r.started_at);
    const use24h = (this.config as any)?.interface?.format_24h ?? false;
    const time = formatTime(r.started_at, use24h);
    const dur = formatDuration(r.duration_ms);
    const cls = statusToClass(r.status);
    const label = statusLabel(r.status);
    const preview = r.transcript ?? truncatedError(r);
    const searchTerm = filterStore.get().search ?? "";

    const cellMap: Record<string, string> = {
      day: `<span class="rec-time">${day}</span>`,
      time: `<span class="rec-time">${time}</span>`,
      duration: `<span class="rec-dur">${dur}</span>`,
      status: `<span class="rec-status"><span class="status-pill ${cls}">${label}</span></span>`,
      transcript: `<span class="rec-preview">${highlightMatch(preview, searchTerm)}</span>`,
    };

    const cells = visibleCols.map((c) => cellMap[c] || "").join("");
    const multiClass = multiChecked ? " multi-selected" : "";

    return `
      <div class="rec-row ${active ? "active" : ""}${kbFocused ? " kbd-focused" : ""}${multiClass}" data-id="${r.id}" role="option" aria-selected="${active}" style="grid-template-columns: ${gridTemplate}">
        <span class="col-checkbox">
          <input
            type="checkbox"
            class="row-cb"
            ${multiChecked ? "checked" : ""}
            aria-label="Select recording from ${new Date(r.started_at).toLocaleString()}"
          />
        </span>
        ${cells}
      </div>
    `;
  }
}

function truncatedError(r: Recording): string {
  if (r.error_message) return `(${r.error_message})`;
  if (r.status === "transcribe_failed") return "(transcription failed)";
  if (r.status === "hook_failed") return "(hook failed)";
  return "(processing…)";
}
