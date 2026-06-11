import { LitElement, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { getRecording, type Recording } from "../../services/ipc";
import { formatTime, formatDuration } from "../../utils/format";
import { trackLabel } from "./grouping";
import "./TranscriptEditor";

/**
 * Side-by-side viewer: two recordings' transcripts in a modal, each in the
 * full transcript editor (vim mode, `:w` save, Save Changes button) — like a
 * vim vsplit over two recordings. Opened from the bulk bar's "Side by side"
 * button or by pressing `\` while exactly two recordings are multi-selected.
 *
 * Escape rules (capture-phase, so the open recording behind never closes):
 * - Focus inside an editor: plain Escape stays with vim (insert → normal);
 *   Shift+Esc steps out of the editor to the dialog (intercepted here so the
 *   app's exit-editor handler doesn't move focus behind the modal).
 * - Otherwise Escape closes the dialog, confirming if either side has unsaved
 *   edits.
 */
@customElement("ph-side-by-side")
export class SideBySideElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for the shared modal styling
  }

  @property({ type: Array }) ids: string[] = [];

  @state() private recs: (Recording | null)[] = [null, null];
  /** Per-pane dirty flags (from the editors' dirty-change events). */
  private dirty = [false, false];

  private keyHandler = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    const inEditor = !!(document.activeElement as HTMLElement | null)?.closest?.("ph-side-by-side .cm-editor");
    if (e.shiftKey) {
      // Shift+Esc: leave the editor but stay in the dialog.
      if (inEditor) {
        e.preventDefault();
        e.stopPropagation();
        (document.activeElement as HTMLElement).blur();
        this.querySelector<HTMLElement>(".sbs-dialog")?.focus();
      }
      return;
    }
    if (inEditor) return; // vim owns plain Escape inside an editor
    e.preventDefault();
    e.stopPropagation();
    void this.close();
  };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("keydown", this.keyHandler, true);
    void this.load();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.keyHandler, true);
  }

  private async load() {
    const [a, b] = this.ids;
    const get = async (id: string) => {
      try {
        return await getRecording(id);
      } catch {
        return null;
      }
    };
    this.recs = await Promise.all([get(a), get(b)]);
  }

  private async close() {
    if (this.dirty.some(Boolean)) {
      const { confirmDialog } = await import("../confirmDialog");
      const discard = await confirmDialog({
        title: "Unsaved changes",
        body: "A transcript here has unsaved edits. Discard them?",
        confirmLabel: "Discard changes",
        cancelLabel: "Keep editing",
        danger: true,
      });
      if (!discard) return;
    }
    this.dispatchEvent(new CustomEvent("closed"));
  }

  private onOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) void this.close();
  }

  private renderPane(rec: Recording | null, idx: number) {
    if (!rec) {
      return html`<div class="sbs-pane"><div class="sbs-loading">Loading…</div></div>`;
    }
    const meta = [
      formatTime(rec.started_at, false),
      formatDuration(rec.duration_ms),
      rec.meeting_name || (rec.meeting_id ? "Meeting" : ""),
      rec.track ? trackLabel(rec.track) : "",
    ]
      .filter(Boolean)
      .join(" · ");
    return html`
      <div class="sbs-pane">
        <div class="sbs-pane-head" title=${rec.id}>${meta}</div>
        <ph-transcript-editor
          class="sbs-editor"
          .recordingId=${rec.id}
          .initialText=${rec.transcript ?? ""}
          .userEdited=${!!rec.user_edited}
          @dirty-change=${(e: Event) => {
            this.dirty[idx] = (e as CustomEvent<boolean>).detail;
          }}
        ></ph-transcript-editor>
      </div>
    `;
  }

  render() {
    return html`
      <style>
        ph-side-by-side .sbs-dialog {
          width: min(1500px, calc(100vw - 48px));
          height: calc(100vh - 80px);
          display: flex;
          flex-direction: column;
          gap: 10px;
          background: var(--bg-elevated);
          border: var(--popup-border);
          border-radius: 12px;
          padding: 16px 18px;
          box-shadow: 0 24px 70px rgba(0, 0, 0, 0.6);
          outline: none;
        }
        ph-side-by-side .sbs-head {
          display: flex;
          align-items: center;
          justify-content: space-between;
          flex: 0 0 auto;
        }
        ph-side-by-side .sbs-title {
          font-size: 14px;
          font-weight: 600;
          color: var(--fg-default);
        }
        ph-side-by-side .sbs-close {
          background: none;
          border: none;
          color: var(--fg-muted);
          font-size: 15px;
          cursor: pointer;
          padding: 2px 8px;
          border-radius: 6px;
        }
        ph-side-by-side .sbs-close:hover {
          color: var(--fg-default);
          background: rgba(255, 255, 255, 0.06);
        }
        ph-side-by-side .sbs-cols {
          flex: 1;
          min-height: 0;
          display: grid;
          grid-template-columns: 1fr 1fr;
          gap: 14px;
        }
        ph-side-by-side .sbs-pane {
          min-width: 0;
          min-height: 0;
          display: flex;
          flex-direction: column;
          gap: 6px;
          border: 1px solid var(--border-subtle);
          border-radius: 10px;
          padding: 10px 12px;
          background: var(--bg-surface);
          overflow: hidden;
        }
        ph-side-by-side .sbs-pane-head {
          flex: 0 0 auto;
          font-size: 12px;
          font-weight: 600;
          color: var(--fg-muted);
          white-space: nowrap;
          overflow: hidden;
          text-overflow: ellipsis;
        }
        ph-side-by-side .sbs-pane ph-transcript-editor {
          flex: 1;
          min-height: 0;
          overflow-y: auto;
        }
        ph-side-by-side .sbs-loading {
          color: var(--fg-faded);
          font-size: 12px;
          padding: 16px;
        }
        ph-side-by-side .sbs-hint {
          flex: 0 0 auto;
          font-size: 11px;
          color: var(--fg-faded);
        }
      </style>
      <div class="modal-overlay" @click=${this.onOverlayClick}>
        <div class="sbs-dialog" role="dialog" aria-modal="true" aria-label="Side-by-side transcripts" tabindex="-1">
          <div class="sbs-head">
            <span class="sbs-title">◫ Side by side</span>
            <button class="sbs-close" title="Close (Esc)" @click=${() => void this.close()}>✕</button>
          </div>
          <div class="sbs-cols">
            ${this.renderPane(this.recs[0], 0)} ${this.renderPane(this.recs[1], 1)}
          </div>
          ${this.recs.some(Boolean)
            ? html`<div class="sbs-hint">Full editors — vim keys and <code>:w</code> work per pane. Shift+Esc leaves an editor; Esc closes.</div>`
            : nothing}
        </div>
      </div>
    `;
  }
}

/** Open the side-by-side viewer over two recordings. Resolves when closed. */
export function openSideBySide(idA: string, idB: string): Promise<void> {
  return new Promise((resolve) => {
    document.querySelector("ph-side-by-side")?.remove();
    const el = document.createElement("ph-side-by-side") as SideBySideElement;
    el.ids = [idA, idB];
    el.addEventListener("closed", () => {
      el.remove();
      resolve();
    });
    document.body.appendChild(el);
  });
}
