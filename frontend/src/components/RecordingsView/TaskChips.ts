import { errText } from "../../utils/error";
import { LitElement, html, PropertyValues, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import {
  getRecording,
  suggestTasks,
  setTaskDone,
  addTask,
  updateTask,
  deleteTask,
  reorderTasks,
  type Task,
} from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { showToast } from "../../utils/toast";
import { enrichHead, loadCollapsed, saveCollapsed } from "./enrichSection";

/**
 * The detail pane's tasks surface: the recording's action items as a full task
 * manager — check off, add by hand, edit, delete, and drag to reorder — plus the
 * ✅ Extract button that runs the LLM task-extraction step on demand.
 *
 * Tasks are INTERACTIVE (unlike the read-only {@link EntityChips}). Each mutation
 * persists via its IPC ({@link setTaskDone} / {@link addTask} / {@link updateTask}
 * / {@link deleteTask} / {@link reorderTasks}) and then reconciles against the
 * daemon's truth (the `tasks_updated` event re-fetches; mutations also reload
 * directly so a write that didn't take can't keep showing as saved). Hand-added
 * ("manual") tasks survive a re-extraction — the daemon only replaces LLM rows.
 *
 * Loads its own data per `recordingId` (off `getRecording`) and live-refreshes on
 * the `tasks_updated` daemon event. Errors toast.
 */
@customElement("ph-task-chips")
export class TaskChipsElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM, to inherit the global tag-chip styles.
  }

  @property({ type: String }) recordingId = "";

  @state() private tasks: Task[] = [];
  /** True while an on-demand ✅ Extract run is in flight. */
  @state() private extracting = false;
  /** Row ids with a toggle in flight, so the checkbox can't double-fire. */
  @state() private toggling = new Set<number>();
  /** Section collapsed (remembered across reloads + recording switches). */
  @state() private collapsed = loadCollapsed("tasks");
  /** Hide completed tasks (per-section, remembered). */
  @state() private hideDone = loadCollapsed("tasks.hideDone");
  /** The task row currently being edited inline, and its draft text. */
  @state() private editingId: number | null = null;
  @state() private editText = "";
  /** The "+ add task" draft. */
  @state() private addText = "";
  /** Drag-reorder state (row being dragged / hovered). */
  private dragId: number | null = null;
  @state() private dragOverId: number | null = null;
  private unsubEvents: (() => void) | null = null;

  private toggleCollapsed = () => {
    this.collapsed = !this.collapsed;
    saveCollapsed("tasks", this.collapsed);
  };
  private toggleHideDone = (e: Event) => {
    e.stopPropagation(); // don't toggle the section collapse
    this.hideDone = !this.hideDone;
    saveCollapsed("tasks.hideDone", this.hideDone);
  };

  connectedCallback() {
    super.connectedCallback();
    if (this.recordingId) void this.load();
    void subscribe((e: DaemonEvent) => {
      if (e.event === "tasks_updated" && e.id === this.recordingId) {
        this.extracting = false;
        void this.load();
      }
      if (e.event === "tasks_failed" && e.id === this.recordingId) {
        this.extracting = false;
      }
    }).then((un) => {
      if (!this.isConnected) un();
      else this.unsubEvents = un;
    });
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.unsubEvents?.();
    this.unsubEvents = null;
  }

  updated(changed: PropertyValues) {
    if (changed.has("recordingId") && this.recordingId) {
      this.editingId = null;
      this.addText = "";
      void this.load();
    }
  }

  private async load() {
    try {
      const rec = await getRecording(this.recordingId);
      this.tasks = rec.tasks ?? [];
    } catch {
      this.tasks = [];
    }
  }

  /** ✅ Extract: ask the LLM for action items for this recording, now. */
  private async runExtract() {
    if (this.extracting) return;
    this.extracting = true;
    try {
      await suggestTasks(this.recordingId);
      await this.load();
    } catch (e) {
      showToast(`Task extraction failed: ${errText(e)}`, "error");
    } finally {
      this.extracting = false;
    }
  }

  /** Toggle one task's done flag — optimistic, reconciled against the daemon's
   *  persisted truth on both paths (see the P0d hardening). */
  private async toggleDone(task: Task) {
    if (this.toggling.has(task.id)) return;
    const next = !task.done;
    this.toggling = new Set(this.toggling).add(task.id);
    this.tasks = this.tasks.map((t) => (t.id === task.id ? { ...t, done: next } : t));
    try {
      await setTaskDone(this.recordingId, task.id, next);
      await this.load();
    } catch (e) {
      showToast(`Could not update task: ${errText(e)}`, "error");
      await this.load();
    } finally {
      const t = new Set(this.toggling);
      t.delete(task.id);
      this.toggling = t;
    }
  }

  /** Add a hand-written task from the "+ add task" field. */
  private async addNew() {
    const text = this.addText.trim();
    if (!text) return;
    this.addText = "";
    try {
      await addTask(this.recordingId, text);
      await this.load();
    } catch (e) {
      this.addText = text; // restore so the user doesn't lose it
      showToast(`Could not add task: ${errText(e)}`, "error");
    }
  }

  private startEdit(task: Task) {
    this.editingId = task.id;
    this.editText = task.text;
  }
  private cancelEdit() {
    this.editingId = null;
    this.editText = "";
  }
  private async saveEdit(task: Task) {
    const text = this.editText.trim();
    if (!text || text === task.text) {
      this.cancelEdit();
      return;
    }
    this.editingId = null;
    try {
      // Preserve the existing due hint; this inline edit only changes the text.
      await updateTask(this.recordingId, task.id, text, task.due_hint ?? null);
      await this.load();
    } catch (e) {
      showToast(`Could not save task: ${errText(e)}`, "error");
      await this.load();
    }
  }

  private async removeTask(task: Task) {
    try {
      await deleteTask(this.recordingId, task.id);
      await this.load();
    } catch (e) {
      showToast(`Could not delete task: ${errText(e)}`, "error");
      await this.load();
    }
  }

  // ── Drag-reorder ──────────────────────────────────────────────────────────
  private onDragStart(task: Task, e: DragEvent) {
    this.dragId = task.id;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(task.id));
    }
  }
  private onDragOver(task: Task, e: DragEvent) {
    if (this.dragId === null || this.dragId === task.id) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    if (this.dragOverId !== task.id) this.dragOverId = task.id;
  }
  private async onDrop(task: Task, e: DragEvent) {
    e.preventDefault();
    const from = this.dragId;
    this.dragId = null;
    this.dragOverId = null;
    if (from === null || from === task.id) return;
    const ids = this.tasks.map((t) => t.id).filter((id) => id !== from);
    const at = ids.indexOf(task.id);
    // Dropping onto a row inserts BEFORE it.
    ids.splice(at, 0, from);
    // Optimistic local reorder for instant feel.
    const byId = new Map(this.tasks.map((t) => [t.id, t]));
    this.tasks = ids.map((id) => byId.get(id)!).filter(Boolean);
    try {
      await reorderTasks(this.recordingId, ids);
      await this.load();
    } catch (err) {
      showToast(`Could not reorder: ${errText(err)}`, "error");
      await this.load();
    }
  }
  private onDragEnd() {
    this.dragId = null;
    this.dragOverId = null;
  }

  private onAddKeydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      void this.addNew();
    }
  }
  private onEditKeydown(task: Task, e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      void this.saveEdit(task);
    } else if (e.key === "Escape") {
      e.preventDefault();
      this.cancelEdit();
    }
  }

  render() {
    const total = this.tasks.length;
    const openCount = this.tasks.filter((t) => !t.done).length;
    const visible = this.hideDone ? this.tasks.filter((t) => !t.done) : this.tasks;
    return html`
      <div class="detail-enrich tasks ${this.collapsed ? "is-collapsed" : ""}">
        ${enrichHead({
          label: "✅ Tasks",
          collapsed: this.collapsed,
          onToggle: this.toggleCollapsed,
          count: total ? html`<span title="${openCount} open of ${total}">${openCount}/${total}</span>` : undefined,
          action: html`${total
              ? html`<button class="tag-manage task-hidedone ${this.hideDone ? "on" : ""}"
                  title=${this.hideDone ? "Show completed tasks" : "Hide completed tasks"}
                  @click=${this.toggleHideDone}>${this.hideDone ? "Show done" : "Hide done"}</button>`
              : nothing}
            <button class="tag-manage task-extract"
              title="Ask the AI to pull concrete action items / to-dos from this recording. Re-running replaces the AI list but keeps any task you added or checked off."
              ?disabled=${this.extracting} @click=${() => void this.runExtract()}>${this.extracting ? "✅ Extracting…" : "✅ Extract"}</button>`,
        })}
        ${this.collapsed
          ? ""
          : html`<div class="enrich-body tasks-list">
              ${visible.map((t) => this.renderRow(t))}
              <div class="task-add-row">
                <span class="task-add-plus" aria-hidden="true">+</span>
                <input
                  class="task-add-input"
                  type="text"
                  placeholder="Add a task…"
                  .value=${this.addText}
                  @input=${(e: Event) => (this.addText = (e.target as HTMLInputElement).value)}
                  @keydown=${(e: KeyboardEvent) => this.onAddKeydown(e)}
                />
                ${this.addText.trim()
                  ? html`<button class="task-add-btn" title="Add task" @click=${() => void this.addNew()}>Add</button>`
                  : nothing}
              </div>
            </div>`}
      </div>
    `;
  }

  private renderRow(t: Task) {
    if (this.editingId === t.id) {
      return html`<div class="task-row task-row--editing">
        <input
          class="task-edit-input"
          type="text"
          .value=${this.editText}
          autofocus
          @input=${(e: Event) => (this.editText = (e.target as HTMLInputElement).value)}
          @keydown=${(e: KeyboardEvent) => this.onEditKeydown(t, e)}
          @blur=${() => void this.saveEdit(t)}
        />
      </div>`;
    }
    return html`<div
      class="task-row ${t.done ? "done" : ""} ${this.dragOverId === t.id ? "task-row--dragover" : ""}"
      draggable="true"
      @dragstart=${(e: DragEvent) => this.onDragStart(t, e)}
      @dragover=${(e: DragEvent) => this.onDragOver(t, e)}
      @drop=${(e: DragEvent) => void this.onDrop(t, e)}
      @dragend=${() => this.onDragEnd()}
    >
      <span class="task-drag-grip" aria-hidden="true" title="Drag to reorder">⠿</span>
      <input type="checkbox" class="task-check" .checked=${t.done}
        ?disabled=${this.toggling.has(t.id)}
        title=${t.done ? "Mark as not done" : "Mark as done"}
        @change=${() => void this.toggleDone(t)} />
      <span class="task-text" @dblclick=${() => this.startEdit(t)} title="Double-click to edit">${t.text}${t.due_hint
        ? html`<span class="task-due">${t.due_hint}</span>`
        : ""}</span>
      <span class="task-row-actions">
        <button class="task-row-btn" title="Edit" aria-label="Edit task" @click=${() => this.startEdit(t)}>✎</button>
        <button class="task-row-btn task-row-btn--del" title="Delete" aria-label="Delete task" @click=${() => void this.removeTask(t)}>✕</button>
      </span>
    </div>`;
  }
}

/** Imperative mount wrapper: RecordingDetail creates one per render; the element
 *  manages its own data from there. Mirrors {@link EntityChips}. */
export class TaskChips {
  private element: TaskChipsElement;
  constructor(container: HTMLElement, recordingId: string) {
    this.element = document.createElement("ph-task-chips") as TaskChipsElement;
    this.element.recordingId = recordingId;
    container.appendChild(this.element);
  }
}
