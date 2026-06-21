import { errText } from "../../utils/error";
import { LitElement, html, PropertyValues } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { getRecording, suggestTasks, setTaskDone, type Task } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { showToast } from "../../utils/toast";

/**
 * The detail pane's tasks surface: the recording's extracted action items
 * rendered as a checkable list, with a ✅ Extract button that runs the LLM
 * task-extraction step on demand.
 *
 * The task counterpart of {@link EntityChips} (which is read-only). The key
 * deviation: tasks are INTERACTIVE — each row has a checkbox bound to `done`, and
 * clicking it calls {@link setTaskDone} so the user owns that one field. Done
 * tasks dim + strike through and sort below open ones (the daemon orders them).
 * A `due_hint` renders as a muted suffix when the model emitted one.
 *
 * Loads its own data per `recordingId` (off `getRecording`) and live-refreshes on
 * the `tasks_updated` daemon event, so a pipeline run, an on-demand extract, or a
 * checkbox toggle elsewhere updates the list. Errors toast.
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
  private unsubEvents: (() => void) | null = null;

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
    if (changed.has("recordingId") && this.recordingId) void this.load();
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
      // The tasks_updated event refreshes the list; this covers the
      // nothing-extracted case (no event fires then).
      await this.load();
    } catch (e) {
      showToast(`Task extraction failed: ${errText(e)}`, "error");
    } finally {
      this.extracting = false;
    }
  }

  /** Toggle one task's done flag. Optimistic — flips locally, then persists; on
   *  failure it reloads to snap back to the daemon's truth and toasts. */
  private async toggleDone(task: Task) {
    if (this.toggling.has(task.id)) return;
    const next = !task.done;
    this.toggling = new Set(this.toggling).add(task.id);
    // Optimistic local flip so the checkbox feels instant.
    this.tasks = this.tasks.map((t) => (t.id === task.id ? { ...t, done: next } : t));
    try {
      await setTaskDone(this.recordingId, task.id, next);
      // The tasks_updated event re-fetches with the daemon's open-first ordering.
    } catch (e) {
      showToast(`Could not update task: ${errText(e)}`, "error");
      await this.load(); // revert to server truth
    } finally {
      const t = new Set(this.toggling);
      t.delete(task.id);
      this.toggling = t;
    }
  }

  render() {
    return html`
      <div class="tasks">
        <div class="tags-row tags-controls">
          <span class="tasks-label" title="Action items the AI pulled from this transcript — check them off as you go"
            style="font-size: 0.7857rem; color: var(--fg-muted);">✅ Tasks</span>
          <button class="tag-manage task-extract"
            title="Ask the AI to pull concrete action items / to-dos from this recording. Re-running replaces the list but keeps any task you already checked off."
            ?disabled=${this.extracting} @click=${() => void this.runExtract()}>${this.extracting ? "✅ Extracting…" : "✅ Extract"}</button>
        </div>
        ${this.tasks.length
          ? html`<ul class="tasks-list" style="list-style:none; margin:6px 0 0; padding:0; display:flex; flex-direction:column; gap:4px;">
              ${this.tasks.map(
                (t) => html`
                  <li class="task-row" style="display:flex; align-items:baseline; gap:8px;">
                    <input type="checkbox" class="task-check" .checked=${t.done}
                      ?disabled=${this.toggling.has(t.id)}
                      title=${t.done ? "Mark as not done" : "Mark as done"}
                      @change=${() => void this.toggleDone(t)} />
                    <span class="task-text" style=${t.done
                      ? "text-decoration:line-through; color:var(--fg-faded);"
                      : ""}>${t.text}${t.due_hint
                        ? html`<span class="task-due" style="margin-left:6px; font-size:0.7857rem; color:var(--fg-muted);">(${t.due_hint})</span>`
                        : ""}</span>
                  </li>
                `,
              )}
            </ul>`
          : html`<div class="tasks-empty" style="font-size: 0.7857rem; color: var(--fg-muted); margin-top:4px;">No tasks yet — Extract to pull action items and to-dos from the transcript.</div>`}
      </div>
    `;
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
