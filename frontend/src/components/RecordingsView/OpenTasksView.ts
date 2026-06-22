import { LitElement, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { listAllTasks, setTaskDone, type TaskWithRecording } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { closeModalOverlay } from "../../utils/modalAnim";
import { showToast } from "../../utils/toast";
import { errText } from "../../utils/error";

/**
 * The global task list — "everything I have to do", every extracted task across
 * the library in one modal: checkable in place, filterable, and each click-through
 * jumps to its source recording. The flat-list companion to the sidebar's Tasks
 * section (which filters the *library* by has-open-tasks); this lists the
 * individual tasks themselves.
 *
 * Opened by the `phoneme:open-tasks` window event (the sidebar Tasks section's
 * "View all…" row). Toggling a checkbox calls {@link setTaskDone}; clicking a
 * task's recording dispatches `phoneme:select-recording` and closes. Esc / overlay
 * / Done close it; live-refreshes on `tasks_updated`.
 */
@customElement("ph-open-tasks")
export class OpenTasksElement extends LitElement {
  protected createRenderRoot() {
    return this; // light DOM for global modal CSS + theme vars
  }

  @state() private openState = false;
  @state() private tasks: TaskWithRecording[] = [];
  @state() private filter = "";
  @state() private openOnly = false;
  private unsub: (() => void) | null = null;

  private onOpen = () => {
    this.openState = true;
    this.toggleAttribute("data-open", true);
    void this.load();
  };
  private keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape" && this.openState) {
      e.stopPropagation();
      this.close();
    }
  };

  async connectedCallback() {
    super.connectedCallback();
    window.addEventListener("phoneme:open-tasks", this.onOpen);
    document.addEventListener("keydown", this.keyHandler);
    const unsub = await subscribe((e: DaemonEvent) => {
      if (!this.openState) return;
      if (e.event === "tasks_updated") void this.load();
    });
    if (!this.isConnected) unsub();
    else this.unsub = unsub;
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("phoneme:open-tasks", this.onOpen);
    document.removeEventListener("keydown", this.keyHandler);
    this.unsub?.();
  }

  private close() {
    const overlay = this.querySelector<HTMLElement>(".modal-overlay");
    const done = () => {
      this.openState = false;
      this.toggleAttribute("data-open", false);
    };
    if (overlay) closeModalOverlay(overlay, done);
    else done();
  }

  private async load() {
    try {
      this.tasks = await listAllTasks(false);
    } catch {
      this.tasks = [];
    }
  }

  private async toggleDone(t: TaskWithRecording) {
    const next = !t.done;
    // Optimistic, then reconcile via the reload the event triggers.
    this.tasks = this.tasks.map((x) =>
      x.recording_id === t.recording_id && x.id === t.id ? { ...x, done: next } : x,
    );
    try {
      await setTaskDone(t.recording_id, t.id, next);
    } catch (e) {
      showToast(`Could not update task: ${errText(e)}`, "error");
      await this.load();
    }
  }

  private goto(t: TaskWithRecording) {
    window.dispatchEvent(new CustomEvent("phoneme:select-recording", { detail: { id: t.recording_id } }));
    this.close();
  }

  private handleOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) this.close();
  }

  render() {
    if (!this.openState) return html``;
    const q = this.filter.trim().toLowerCase();
    const rows = this.tasks.filter(
      (t) =>
        (!this.openOnly || !t.done) &&
        (!q || t.text.toLowerCase().includes(q) || (t.title ?? "").toLowerCase().includes(q)),
    );
    const openCount = this.tasks.filter((t) => !t.done).length;
    return html`
      <div class="modal-overlay" @click=${this.handleOverlayClick}>
        <div class="modal-dialog open-tasks-dialog" role="dialog" aria-modal="true" aria-labelledby="open-tasks-title">
          <div class="modal-header open-tasks-header">
            <span class="modal-icon" aria-hidden="true">📋</span>
            <div class="open-tasks-head-text">
              <h3 class="modal-title" id="open-tasks-title">All tasks</h3>
              <span class="open-tasks-subtitle">${openCount} open · ${this.tasks.length} total across your library</span>
            </div>
            <button class="ask-close" @click=${() => this.close()} title="Close (Esc)" aria-label="Close">✕</button>
          </div>

          <div class="open-tasks-controls">
            <input class="open-tasks-search" type="text" placeholder="Filter tasks…"
              .value=${this.filter}
              @input=${(e: Event) => (this.filter = (e.target as HTMLInputElement).value)} />
            <label class="open-tasks-toggle" title="Hide tasks you've completed">
              <input type="checkbox" .checked=${this.openOnly}
                @change=${(e: Event) => (this.openOnly = (e.target as HTMLInputElement).checked)} />
              Open only
            </label>
          </div>

          <div class="open-tasks-body">
            ${rows.length === 0
              ? html`<div class="open-tasks-empty">${this.tasks.length === 0 ? "No tasks yet. Extract them from a recording's detail view." : "Nothing matches."}</div>`
              : rows.map((t) => this.renderRow(t))}
          </div>

          <div class="modal-actions">
            <button class="modal-btn" @click=${() => this.close()}>Done</button>
          </div>
        </div>
      </div>
    `;
  }

  private renderRow(t: TaskWithRecording) {
    return html`<div class="open-task-row ${t.done ? "is-done" : ""}">
      <input type="checkbox" class="open-task-check" .checked=${t.done}
        @change=${() => void this.toggleDone(t)} title="${t.done ? "Mark not done" : "Mark done"}" />
      <div class="open-task-main">
        <span class="open-task-text">${t.text}</span>
        ${t.due_hint ? html`<span class="open-task-due">${t.due_hint}</span>` : nothing}
      </div>
      <button class="open-task-src" title="Open this recording"
        @click=${() => this.goto(t)}>${t.title?.trim() || "Untitled"} ↗</button>
    </div>`;
  }
}
