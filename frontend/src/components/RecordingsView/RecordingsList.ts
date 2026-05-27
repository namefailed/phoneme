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
  escapeHtml,
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
  /** Cached app config — loaded once and refreshed when settings are saved. */
  private config: any = null;
  private currentWidths: string[] | null = null;

  constructor(
    container: HTMLElement,
    state: Store<RecordingsListState>,
    onSelect: (id: string) => void,
  ) {
    this.container = container;
    this.state = state;
    this.onSelect = onSelect;
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
      this.state.set({ ...this.state.get(), recordings: rows, loading: false });
    } catch (e) {
      this.state.set({ ...this.state.get(), error: String(e), loading: false });
    }
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

    const gridTemplate = this.currentWidths.join(" ");

    const headSpans = visibleCols
      .map(
        (c, i) => `
      <span class="col-head" data-col="${i}">
        ${colLabels[c] || c}
        ${i < visibleCols.length - 1 ? `<div class="resizer" data-col="${i}"></div>` : ""}
      </span>
    `,
      )
      .join("");

    const head = `
      <div class="rec-table-head" style="grid-template-columns: ${gridTemplate}">
        ${headSpans}
      </div>
    `;

    const rows = s.recordings
      .map((r) => this.renderRow(r, r.id === s.selectedId, visibleCols, gridTemplate))
      .join("");
    this.container.innerHTML = `<div class="rec-table">${head}${rows}</div>`;

    this.container.querySelectorAll<HTMLElement>(".rec-row").forEach((el) => {
      el.addEventListener("click", () => {
        const id = el.getAttribute("data-id");
        if (id) this.onSelect(id);
      });
    });

    this.container.querySelectorAll<HTMLElement>(".resizer").forEach((el) => {
      el.addEventListener("mousedown", (e: MouseEvent) => {
        e.preventDefault();
        e.stopPropagation();

        const colIdx = parseInt(el.getAttribute("data-col") || "0", 10);
        const startX = e.clientX;
        const heads = Array.from(this.container.querySelectorAll(".col-head"));
        const startW = (heads[colIdx] as HTMLElement).offsetWidth;

        const onMove = (moveEvent: MouseEvent) => {
          if (!this.currentWidths) return;
          const newW = Math.max(30, startW + moveEvent.clientX - startX);
          this.currentWidths[colIdx] = `${newW}px`;
          const template = this.currentWidths.join(" ");
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

  private renderRow(
    r: Recording,
    active: boolean,
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

    const cellMap: Record<string, string> = {
      day: `<span class="rec-time">${day}</span>`,
      time: `<span class="rec-time">${time}</span>`,
      duration: `<span class="rec-dur">${dur}</span>`,
      status: `<span class="rec-status"><span class="status-pill ${cls}">${label}</span></span>`,
      transcript: `<span class="rec-preview">${escapeHtml(preview)}</span>`,
    };

    const cells = visibleCols.map((c) => cellMap[c] || "").join("");

    return `
      <div class="rec-row ${active ? "active" : ""}" data-id="${r.id}" style="grid-template-columns: ${gridTemplate}">
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
