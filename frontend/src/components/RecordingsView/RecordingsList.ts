import { listRecordings, type Recording } from "../../services/ipc";
import { Store } from "../../state/store";
import { filterStore } from "../../state/filter";
import { invoke } from "@tauri-apps/api/core";

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
  private config: any = null;

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
  }

  async refresh() {
    this.state.set({ ...this.state.get(), loading: true, error: null });
    try {
      const f = filterStore.get();
      const [rows, cfg] = await Promise.all([
        listRecordings({ ...f, limit: 200 }),
        invoke<any>("read_config")
      ]);
      this.config = cfg;
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

    const visibleCols: string[] = this.config?.tray?.visible_columns || [
      "time", "duration", "status", "transcript"
    ];

    const colLabels: Record<string, string> = {
      day: "Day",
      time: "Time",
      duration: "Dur",
      status: "Status",
      transcript: "Transcript"
    };

    const colWidths: Record<string, string> = {
      day: "75px",
      time: "70px",
      duration: "60px",
      status: "40px",
      transcript: "1fr"
    };

    const gridTemplate = visibleCols.map(c => colWidths[c] || "auto").join(" ");

    const headSpans = visibleCols.map(c => `<span>${colLabels[c] || c}</span>`).join("");
    const head = `
      <div class="rec-table-head" style="grid-template-columns: ${gridTemplate}">
        ${headSpans}
      </div>
    `;

    const rows = s.recordings.map((r) => this.renderRow(r, r.id === s.selectedId, visibleCols, gridTemplate)).join("");
    this.container.innerHTML = `<div class="rec-table">${head}${rows}</div>`;

    this.container.querySelectorAll<HTMLElement>(".rec-row").forEach((el) => {
      el.addEventListener("click", () => {
        const id = el.getAttribute("data-id");
        if (id) this.onSelect(id);
      });
    });
  }

  private renderRow(r: Recording, active: boolean, visibleCols: string[], gridTemplate: string): string {
    const day = formatDay(r.started_at);
    const time = formatTime(r.started_at);
    const dur = formatDuration(r.duration_ms);
    const statusClass = statusToClass(r.status);
    const preview = (r.transcript ?? truncatedError(r));

    const cellMap: Record<string, string> = {
      day: `<span class="rec-time">${day}</span>`,
      time: `<span class="rec-time">${time}</span>`,
      duration: `<span class="rec-dur">${dur}</span>`,
      status: `<span class="rec-status"><span class="status-dot ${statusClass}"></span></span>`,
      transcript: `<span class="rec-preview">${escapeHtml(preview)}</span>`
    };

    const cells = visibleCols.map(c => cellMap[c] || "").join("");

    return `
      <div class="rec-row ${active ? "active" : ""}" data-id="${r.id}" style="grid-template-columns: ${gridTemplate}">
        ${cells}
      </div>
    `;
  }
}

function formatDay(iso: string): string {
  const d = new Date(iso);
  const today = new Date();
  const isToday = d.getFullYear() === today.getFullYear() &&
                  d.getMonth() === today.getMonth() &&
                  d.getDate() === today.getDate();
  if (isToday) return "Today";
  
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  const isYesterday = d.getFullYear() === yesterday.getFullYear() &&
                      d.getMonth() === yesterday.getMonth() &&
                      d.getDate() === yesterday.getDate();
  if (isYesterday) return "Yest.";

  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

function formatDuration(ms: number): string {
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60_000)}m${Math.floor((ms % 60_000) / 1000)
    .toString()
    .padStart(2, "0")}s`;
}

function statusToClass(status: string): string {
  if (status === "done") return "done";
  if (status === "transcribe_failed" || status === "hook_failed") return "failed";
  return "pending";
}

function truncatedError(r: Recording): string {
  if (r.error_message) return `(${r.error_message})`;
  if (r.status === "transcribe_failed") return "(transcription failed)";
  if (r.status === "hook_failed") return "(hook failed)";
  return "(processing…)";
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
