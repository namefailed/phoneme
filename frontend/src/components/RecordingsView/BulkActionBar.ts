import { LitElement, html, css, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { deleteRecording, retranscribeRecording, type Recording } from "../../services/ipc";
import { showToast } from "../../utils/toast";

export type BulkActionCallbacks = {
  onRefresh: () => void;
  onClear: () => void;
};

@customElement('ph-bulk-action-bar')
export class BulkActionBarElement extends LitElement {
  protected createRenderRoot() {
    return this; // Use light DOM to inherit global styling like .bulk-bar
  }

  @property({ type: Object }) selected: ReadonlySet<string> = new Set();
  @property({ type: Array }) recordings: ReadonlyArray<Recording> = [];
  @property({ type: Object }) callbacks!: BulkActionCallbacks;

  @state() private busy = false;

  private selectedRecordings(): Recording[] {
    return this.recordings.filter((r) => this.selected.has(r.id));
  }

  updated(changedProperties: PropertyValues) {
    if (changedProperties.has('selected')) {
      const display = this.selected.size > 0 ? "flex" : "none";
      if (this.style.display !== display) {
        this.style.display = display;
      }
    }
  }

  private async handleRetranscribe() {
    if (this.busy) return;
    this.busy = true;
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
    this.busy = false;
    if (failed === 0) {
      showToast(`Queued ${ok} recording${ok !== 1 ? "s" : ""} for re-transcription.`, "success");
    } else {
      showToast(`${ok} queued, ${failed} failed.`, "error");
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

  private async handleDelete() {
    if (this.busy) return;
    const recs = this.selectedRecordings();
    const n = recs.length;
    
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

    this.busy = true;
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
    this.busy = false;
    
    if (failed === 0) {
      showToast(`Deleted ${ok} recording${ok !== 1 ? "s" : ""}.`, "success");
    } else {
      showToast(`${ok} deleted, ${failed} failed.`, "error");
    }
    this.callbacks.onClear();
    this.callbacks.onRefresh();
  }

  private handleClear() {
    this.callbacks.onClear();
  }

  render() {
    const n = this.selected.size;
    if (n === 0) return html``;

    const label = n === 1 ? "1 recording selected" : `${n} recordings selected`;

    return html`
      <div class="bulk-bar">
        <span class="bulk-count">${this.busy ? "Working…" : label}</span>
        <div class="bulk-actions">
          <button class="bulk-btn" title="Re-transcribe selected recordings" .disabled=${this.busy} @click=${this.handleRetranscribe}>
            ↺ Re-transcribe
          </button>
          <button class="bulk-btn" title="Export transcripts as plain text" .disabled=${this.busy} @click=${this.handleExport}>
            ↓ Export
          </button>
          <button class="bulk-btn bulk-btn--danger" title="Delete selected recordings" .disabled=${this.busy} @click=${this.handleDelete}>
            🗑 Delete
          </button>
          <button class="bulk-btn bulk-btn--muted" title="Deselect all" .disabled=${this.busy} @click=${this.handleClear}>
            ✕ Deselect
          </button>
        </div>
      </div>
    `;
  }
}

// Temporary vanilla wrapper
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

  // Called manually by parent right now
  update(selected: ReadonlySet<string>, recordings: ReadonlyArray<Recording>) {
    this.element.selected = selected;
    this.element.recordings = recordings;
  }
}
