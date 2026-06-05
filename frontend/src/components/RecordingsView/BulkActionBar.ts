/**
 * BulkActionBar — floating action bar that appears when ≥1 recordings are
 * multi-selected in the recordings list.
 *
 * It mounts itself into the given `container` and triggers the provided
 * callbacks. The parent is responsible for clearing selection on success.
 */
import {
  deleteRecording,
  retranscribeRecording,
  type Recording,
} from "../../services/ipc";
import { showToast } from "../../utils/toast";

export type BulkActionCallbacks = {
  /** Called after any operation that requires a list refresh. */
  onRefresh: () => void;
  /** Called when the user clicks "Deselect all". */
  onClear: () => void;
};

export class BulkActionBar {
  private container: HTMLElement;
  private selected: ReadonlySet<string>;
  private recordings: ReadonlyArray<Recording>;
  private callbacks: BulkActionCallbacks;
  private busy = false;

  constructor(
    container: HTMLElement,
    selected: ReadonlySet<string>,
    recordings: ReadonlyArray<Recording>,
    callbacks: BulkActionCallbacks,
  ) {
    this.container = container;
    this.selected = selected;
    this.recordings = recordings;
    this.callbacks = callbacks;
    this.render();
  }

  private selectedRecordings(): Recording[] {
    return this.recordings.filter((r) => this.selected.has(r.id));
  }

  private render() {
    const n = this.selected.size;
    if (n === 0) {
      this.container.innerHTML = "";
      this.container.style.display = "none";
      return;
    }

    this.container.style.display = "flex";
    const label = n === 1 ? "1 recording selected" : `${n} recordings selected`;

    this.container.innerHTML = `
      <div class="bulk-bar">
        <span class="bulk-count">${label}</span>
        <div class="bulk-actions">
          <button id="bulk-retranscribe" class="bulk-btn" title="Re-transcribe selected recordings">
            ↺ Re-transcribe
          </button>
          <button id="bulk-export" class="bulk-btn" title="Export transcripts as plain text">
            ↓ Export
          </button>
          <button id="bulk-delete" class="bulk-btn bulk-btn--danger" title="Delete selected recordings">
            🗑 Delete
          </button>
          <button id="bulk-clear" class="bulk-btn bulk-btn--muted" title="Deselect all">
            ✕ Deselect
          </button>
        </div>
      </div>
    `;

    this.container.querySelector("#bulk-retranscribe")?.addEventListener("click", () => {
      void this.handleRetranscribe();
    });
    this.container.querySelector("#bulk-export")?.addEventListener("click", () => {
      void this.handleExport();
    });
    this.container.querySelector("#bulk-delete")?.addEventListener("click", () => {
      void this.handleDelete();
    });
    this.container.querySelector("#bulk-clear")?.addEventListener("click", () => {
      this.callbacks.onClear();
    });
  }

  private async handleRetranscribe() {
    if (this.busy) return;
    this.setBusy(true);
    const recs = this.selectedRecordings();
    let ok = 0;
    let failed = 0;
    for (const r of recs) {
      try {
        await retranscribeRecording(r.id);
        ok++;
      } catch {
        failed++;
      }
    }
    this.setBusy(false);
    if (failed === 0) {
      showToast(
        `Queued ${ok} recording${ok !== 1 ? "s" : ""} for re-transcription.`,
        "success",
      );
    } else {
      showToast(
        `${ok} queued, ${failed} failed.`,
        "error",
      );
    }
    this.callbacks.onClear();
    this.callbacks.onRefresh();
  }

  private async handleDelete() {
    if (this.busy) return;
    const recs = this.selectedRecordings();
    const n = recs.length;
    const confirmed = await import("../ConfirmDelete").then(
      ({ confirmDelete }) =>
        confirmDelete({
          title: n === 1 ? "Delete Recording?" : `Delete ${n} Recordings?`,
          body:
            n === 1
              ? "This will permanently delete the recording and its audio file. This action cannot be undone."
              : `This will permanently delete ${n} recordings and their audio files. This action cannot be undone.`,
          confirmLabel: n === 1 ? "Delete" : `Delete ${n} Recordings`,
          skipKey: "phoneme_skip_bulk_delete_confirm",
        }),
    );
    if (!confirmed) return;

    this.setBusy(true);
    let ok = 0;
    let failed = 0;
    for (const r of recs) {
      try {
        await deleteRecording(r.id, false);
        ok++;
      } catch {
        failed++;
      }
    }
    this.setBusy(false);
    if (failed === 0) {
      showToast(
        `Deleted ${ok} recording${ok !== 1 ? "s" : ""}.`,
        "success",
      );
    } else {
      showToast(
        `${ok} deleted, ${failed} failed.`,
        "error",
      );
    }
    this.callbacks.onClear();
    this.callbacks.onRefresh();
  }

  private async handleExport() {
    if (this.busy) return;
    const recs = this.selectedRecordings();

    const lines: string[] = [];
    for (const r of recs) {
      if (!r.transcript) continue;
      const date = new Date(r.started_at).toLocaleString();
      lines.push(`=== ${date} (${r.id}) ===`);
      lines.push(r.transcript.trim());
      lines.push("");
    }

    if (lines.length === 0) {
      showToast("No transcripts to export in the selection.", "error");
      return;
    }

    const blob = new Blob([lines.join("\n")], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `phoneme-export-${Date.now()}.txt`;
    a.click();
    URL.revokeObjectURL(url);
    showToast(`Exported ${recs.filter((r) => r.transcript).length} transcript(s).`, "success");
  }

  private setBusy(busy: boolean) {
    this.busy = busy;
    this.container.querySelectorAll<HTMLButtonElement>(".bulk-btn").forEach((btn) => {
      btn.disabled = busy;
    });
    const countEl = this.container.querySelector<HTMLElement>(".bulk-count");
    if (countEl) {
      countEl.textContent = busy
        ? "Working…"
        : `${this.selected.size} recording${this.selected.size !== 1 ? "s" : ""} selected`;
    }
  }
}
