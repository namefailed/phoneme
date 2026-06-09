import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { listQueue, cancelQueued, type QueueEntry } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { formatTime, formatDuration } from "../../utils/format";
import { showToast } from "../../utils/toast";
import { errText } from "../../utils/error";

/**
 * The transcription pipeline queue, shown pinned to the bottom of the sidebar.
 * Lists the item currently transcribing plus everything waiting, with a cancel
 * button on pending items. Refreshes on any queue-affecting daemon event.
 */
@customElement("ph-queue-panel")
export class QueuePanelElement extends LitElement {
  protected createRenderRoot() {
    return this; // light DOM for global CSS
  }

  @state() private items: QueueEntry[] = [];
  @state() private collapsed = false;
  private unsub: (() => void) | null = null;

  async connectedCallback() {
    super.connectedCallback();
    this.collapsed = localStorage.getItem("phoneme.queuePanelCollapsed") === "true";
    void this.load();
    this.unsub = await subscribe((event: DaemonEvent) => {
      const name = (event as { event: string }).event;
      if (
        name === "queue_depth_changed" ||
        name === "recording_stopped" ||
        name === "transcription_started" ||
        name === "transcription_done" ||
        name === "transcription_failed" ||
        name === "recording_cancelled" ||
        name === "recording_deleted"
      ) {
        void this.load();
      }
    });
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsub) this.unsub();
  }

  private async load() {
    try {
      this.items = await listQueue();
    } catch {
      this.items = [];
    }
  }

  private async cancel(id: string) {
    try {
      await cancelQueued(id);
      await this.load();
    } catch (e) {
      showToast(`Couldn't cancel: ${errText(e)}`, "error");
    }
  }

  private toggle() {
    this.collapsed = !this.collapsed;
    localStorage.setItem("phoneme.queuePanelCollapsed", String(this.collapsed));
  }

  render() {
    const items = this.items;
    const active = items.filter((i) => i.state === "processing").length;
    const pending = items.length - active;
    return html`
      <div class="queue-panel ${this.collapsed ? "collapsed" : "expanded"}">
        <div class="queue-header" @click=${() => this.toggle()} title="Transcription pipeline queue">
          <span class="queue-chevron ${this.collapsed ? "" : "open"}">▸</span>
          <span class="queue-title">Queue</span>
          ${items.length
            ? html`<span class="queue-count">${pending}${active ? ` +${active}⟳` : ""}</span>`
            : html`<span class="queue-count idle">idle</span>`}
        </div>
        ${this.collapsed
          ? null
          : html`
              <div class="queue-list">
                ${items.length === 0
                  ? html`<div class="queue-empty">Nothing queued — recordings transcribe as they arrive.</div>`
                  : items.map(
                      (it) => html`
                        <div class="queue-item ${it.state}">
                          <span class="queue-item-icon">${it.state === "processing" ? "⟳" : "•"}</span>
                          <div class="queue-item-main">
                            <div class="queue-item-title">${formatTime(it.timestamp, false)} · ${formatDuration(it.duration_ms)}</div>
                            <div class="queue-item-sub">${it.state === "processing" ? "Transcribing…" : "Queued"}</div>
                          </div>
                          ${it.state === "pending"
                            ? html`<button class="queue-cancel" title="Remove from queue" @click=${() => this.cancel(it.id)}>✕</button>`
                            : html`<span class="queue-spin" aria-hidden="true"></span>`}
                        </div>
                      `,
                    )}
              </div>
            `}
      </div>
    `;
  }
}
