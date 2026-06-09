import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { listQueue, cancelQueued, reorderQueue, setQueuePaused, queuePaused, cancelAllQueued, getRecording, type QueueEntry } from "../../services/ipc";
import { subscribe, stageLabel, type DaemonEvent, type PipelineStage } from "../../services/events";
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
  /** Per-device override for the queue list's max height (px); null = CSS default. */
  @state() private listHeight: number | null = null;
  /** Whether the daemon's queue worker is paused (gated from claiming new work). */
  @state() private paused = false;
  private unsub: (() => void) | null = null;
  private pollTimer: number | null = null;

  /** Live, non-terminal pipeline stage per recording id (drives the stage
   *  label AND lets re-runs that aren't in the inbox queue still show here). */
  private stages = new Map<string, PipelineStage>();
  /** Synthetic queue rows for active stage items NOT in the inbox (e.g. a
   *  cleanup/summary re-run), keyed by id; fetched on first stage event. */
  private extraEntries = new Map<string, QueueEntry>();

  /** Min/max drag bounds for the queue list height. */
  private static readonly MIN_H = 120;

  /** Forget any live-stage tracking for a recording (terminal/completed). */
  private clearStage(id: string) {
    let changed = false;
    if (this.stages.delete(id)) changed = true;
    if (this.extraEntries.delete(id)) changed = true;
    if (changed) this.requestUpdate();
  }

  async connectedCallback() {
    super.connectedCallback();
    this.collapsed = localStorage.getItem("phoneme.queuePanelCollapsed") === "true";
    const h = Number(localStorage.getItem("phoneme.queueListHeight"));
    this.listHeight = Number.isFinite(h) && h >= QueuePanelElement.MIN_H ? h : null;
    void this.load();
    this.unsub = await subscribe((event: DaemonEvent) => {
      if (event.event === "pipeline_stage_changed") {
        void this.onStageChanged(event.id, event.stage);
        return;
      }
      // A re-run's stage tracking ends when its completion event fires (these
      // don't all go through the inbox, so the stage map is how re-runs clear).
      if (
        event.event === "transcript_updated" ||
        event.event === "summary_updated" ||
        event.event === "summary_failed" ||
        event.event === "hook_done" ||
        event.event === "hook_failed"
      ) {
        this.clearStage(event.id);
      }
      if (
        event.event === "queue_depth_changed" ||
        event.event === "recording_stopped" ||
        event.event === "transcription_started" ||
        event.event === "transcription_done" ||
        event.event === "transcription_failed" ||
        event.event === "recording_cancelled" ||
        event.event === "recording_deleted"
      ) {
        if ("id" in event) this.clearStage(event.id);
        void this.load();
      }
    });
    // Belt-and-suspenders: a light poll so the queue stays fresh even if an
    // event is missed (rapid enqueues, claim races, etc.). Cheap local IPC.
    this.pollTimer = window.setInterval(() => void this.load(), 3000);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsub) this.unsub();
    if (this.pollTimer !== null) clearInterval(this.pollTimer);
  }

  private async load() {
    try {
      this.items = await listQueue();
    } catch {
      this.items = [];
    }
    try {
      this.paused = await queuePaused();
    } catch {
      /* leave last-known paused state */
    }
  }

  /** Handle a live pipeline-stage event: update the stage label and, for an
   *  active item that isn't an inbox row (a cleanup/summary/hook re-run), add a
   *  synthetic row so the re-run shows in the queue too. */
  private async onStageChanged(id: string, stage: PipelineStage) {
    if (stage === "done" || stage === "failed") {
      this.clearStage(id);
      return;
    }
    this.stages.set(id, stage);
    this.requestUpdate();
    // If this id isn't already an inbox row and we haven't fetched it yet,
    // pull its basics so we can render a labeled row for the re-run.
    if (!this.items.some((i) => i.id === id) && !this.extraEntries.has(id)) {
      try {
        const r = await getRecording(id);
        this.extraEntries.set(id, {
          id: r.id,
          timestamp: r.started_at,
          audio_path: r.audio_path,
          duration_ms: r.duration_ms,
          model: r.model ?? "",
          state: "processing",
        });
        this.requestUpdate();
      } catch {
        /* recording vanished — ignore */
      }
    }
  }

  /** Toggle the queue between paused and running. */
  private async togglePause(e: Event) {
    e.stopPropagation();
    const next = !this.paused;
    this.paused = next; // optimistic
    try {
      this.paused = await setQueuePaused(next);
    } catch (err) {
      this.paused = !next;
      showToast(`Couldn't ${next ? "pause" : "resume"} queue: ${errText(err)}`, "error");
    }
  }

  /** Remove every pending item from the queue (the in-flight one keeps going). */
  private async clearAll(e: Event) {
    e.stopPropagation();
    const pending = this.items.filter((i) => i.state === "pending").length;
    if (pending === 0) return;
    if (!window.confirm(`Remove ${pending} pending recording${pending === 1 ? "" : "s"} from the queue? The one currently transcribing will finish.`)) {
      return;
    }
    try {
      const removed = await cancelAllQueued();
      await this.load();
      showToast(`Cleared ${removed} from the queue`, "info");
    } catch (err) {
      showToast(`Couldn't clear queue: ${errText(err)}`, "error");
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

  /** Move a pending item up (-1) or down (+1) and persist the new claim order. */
  private async move(id: string, dir: -1 | 1) {
    const pending = this.items.filter((i) => i.state === "pending");
    const idx = pending.findIndex((i) => i.id === id);
    const j = idx + dir;
    if (idx < 0 || j < 0 || j >= pending.length) return;
    [pending[idx], pending[j]] = [pending[j], pending[idx]];
    // Optimistic: reflect immediately; the poll/event reconciles.
    this.items = [...this.items.filter((i) => i.state === "processing"), ...pending];
    try {
      await reorderQueue(pending.map((i) => i.id));
    } catch (e) {
      showToast(`Couldn't reorder: ${errText(e)}`, "error");
      void this.load();
    }
  }

  /** Open this recording in the main list to watch its progress. */
  private select(id: string) {
    window.dispatchEvent(new CustomEvent("phoneme:select-recording", { detail: { id } }));
  }

  private toggle() {
    this.collapsed = !this.collapsed;
    try { localStorage.setItem("phoneme.queuePanelCollapsed", String(this.collapsed)); } catch { /* private mode */ }
  }

  /**
   * Drag the top splitter to resize the queue list. The panel is laid out
   * column-reverse (list grows upward), so dragging UP must INCREASE the
   * height — hence `startH - dy`. Height is clamped and persisted per device.
   */
  private startResize(e: MouseEvent) {
    e.preventDefault();
    const startY = e.clientY;
    const list = this.querySelector<HTMLElement>(".queue-list");
    const startH = list ? list.getBoundingClientRect().height : QueuePanelElement.MIN_H;
    document.body.style.cursor = "row-resize";
    document.body.style.userSelect = "none";
    const onMove = (m: MouseEvent) => {
      const maxH = Math.max(QueuePanelElement.MIN_H, window.innerHeight * 0.8);
      const next = Math.min(maxH, Math.max(QueuePanelElement.MIN_H, startH - (m.clientY - startY)));
      this.listHeight = Math.round(next);
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      if (this.listHeight != null) {
        try { localStorage.setItem("phoneme.queueListHeight", String(this.listHeight)); } catch { /* private mode */ }
      }
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }

  render() {
    // Merge inbox rows with synthetic rows for active re-runs (cleanup/summary/
    // hook) that aren't in the inbox, so every running step shows in the queue.
    const extras = [...this.extraEntries.values()].filter((e) => !this.items.some((i) => i.id === e.id));
    const items = [...this.items, ...extras];
    const active = items.filter((i) => i.state === "processing").length;
    const pending = items.length - active;
    return html`
      <div class="queue-panel ${this.collapsed ? "collapsed" : "expanded"}">
        <div class="queue-header" @click=${() => this.toggle()} title="Transcription pipeline queue">
          <span class="queue-chevron ${this.collapsed ? "" : "open"}" aria-hidden="true"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg></span>
          <span class="queue-title">Queue</span>
          ${items.length
            ? html`<span class="queue-count">${pending}${active ? ` +${active}⟳` : ""}</span>`
            : html`<span class="queue-count ${this.paused ? "paused" : "idle"}">${this.paused ? "paused" : "idle"}</span>`}
          ${!this.collapsed
            ? html`
                <span class="queue-actions">
                  <button class="queue-action ${this.paused ? "active" : ""}" title=${this.paused ? "Resume the queue" : "Pause the queue (finishes the current item)"}
                    @click=${(e: Event) => this.togglePause(e)}>${this.paused ? "▶" : "⏸"}</button>
                  <button class="queue-action" title="Clear all pending items" ?disabled=${pending === 0}
                    @click=${(e: Event) => this.clearAll(e)}>🗑</button>
                </span>`
            : null}
        </div>
        ${this.collapsed
          ? null
          : html`
              <div class="queue-list" style=${this.listHeight != null ? `max-height:${this.listHeight}px` : ""}>
                ${(() => {
                  const pendingIds = items.filter((i) => i.state === "pending").map((i) => i.id);
                  return items.length === 0
                    ? html`<div class="queue-empty">Nothing queued — recordings transcribe as they arrive.</div>`
                    : items.map((it) => {
                        const pIdx = pendingIds.indexOf(it.id);
                        return html`
                          <div class="queue-item ${it.state}">
                            <span class="queue-item-icon">${it.state === "processing" ? "⟳" : "•"}</span>
                            <div class="queue-item-main" title="Open this recording" @click=${() => this.select(it.id)}>
                              <div class="queue-item-title">${formatTime(it.timestamp, false)} · ${formatDuration(it.duration_ms)}</div>
                              <div class="queue-item-sub">${this.stages.has(it.id) ? stageLabel(this.stages.get(it.id)!) : (it.state === "processing" ? "Transcribing…" : "Queued")}</div>
                            </div>
                            ${it.state === "pending"
                              ? html`
                                  <span class="queue-reorder">
                                    <button class="queue-move" title="Move up" ?disabled=${pIdx <= 0} @click=${() => this.move(it.id, -1)}><svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 15 12 9 18 15"></polyline></svg></button>
                                    <button class="queue-move" title="Move down" ?disabled=${pIdx === pendingIds.length - 1} @click=${() => this.move(it.id, 1)}><svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>
                                  </span>
                                  <button class="queue-cancel" title="Remove from queue" @click=${() => this.cancel(it.id)}>✕</button>
                                `
                              : html`<span class="queue-spin" aria-hidden="true"></span>`}
                          </div>
                        `;
                      });
                })()}
              </div>
              ${items.length
                ? html`<div class="queue-resizer" title="Drag to resize the queue" @mousedown=${(e: MouseEvent) => this.startResize(e)}></div>`
                : null}
            `}
      </div>
    `;
  }
}
