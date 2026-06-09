import { LitElement, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { subscribe, stageLabel, type DaemonEvent, type PipelineStage } from "../../services/events";

type StageActivity = { prompt: string; response: string; done: boolean };

/**
 * A draggable floating popout showing the live AI "thinking" for the selected
 * recording: the exact prompt sent to the LLM and the response as it streams
 * (token-by-token for Ollama; whole-response for non-streaming providers).
 *
 * Mounted once by RecordingsView; `recordingId` tracks the current selection so
 * clicking a queue item (which selects it) shows that file's activity. Listens
 * for `llm_activity` daemon events.
 */
@customElement("ph-thinking-popout")
export class ThinkingPopoutElement extends LitElement {
  protected createRenderRoot() {
    return this; // light DOM for global CSS / theme vars
  }

  @property({ type: String }) recordingId = "";

  @state() private open = false;
  /** Whether activity is currently streaming (drives the button's "live" glow). */
  @state() private live = false;
  /** Per-stage prompt + accumulated response for the current recording. */
  @state() private stages = new Map<PipelineStage, StageActivity>();
  /** Panel floating position; null = anchored to the FAB. */
  @state() private pos: { x: number; y: number } | null = null;
  /** FAB (button) position, draggable + persisted; null = default bottom-right. */
  @state() private fabPos: { x: number; y: number } | null = null;

  private unsub: (() => void) | null = null;
  private static readonly FAB_LS = "phoneme.thinkingFabPos";

  async connectedCallback() {
    super.connectedCallback();
    try {
      const raw = localStorage.getItem(ThinkingPopoutElement.FAB_LS);
      if (raw) {
        const p = JSON.parse(raw);
        if (typeof p?.x === "number" && typeof p?.y === "number") this.fabPos = p;
      }
    } catch { /* ignore */ }
    this.unsub = await subscribe((event: DaemonEvent) => {
      if (event.event !== "llm_activity") return;
      if (event.id !== this.recordingId) return;
      const cur = this.stages.get(event.stage) ?? { prompt: "", response: "", done: false };
      if (event.prompt) cur.prompt = event.prompt;
      if (event.delta) cur.response += event.delta;
      if (event.done) cur.done = true;
      this.stages.set(event.stage, cur);
      this.live = !event.done || [...this.stages.values()].some((s) => !s.done);
      this.requestUpdate();
    });
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsub) this.unsub();
  }

  updated(changed: Map<string, unknown>) {
    // Reset accumulated activity when the selection changes.
    if (changed.has("recordingId")) {
      this.stages = new Map();
      this.live = false;
    }
  }

  /** Current FAB top-left (resolved default = bottom-right with a 20px margin). */
  private fabXY(): { x: number; y: number } {
    return this.fabPos ?? { x: window.innerWidth - 60, y: window.innerHeight - 60 };
  }

  /** Drag the panel by its header (fine-tune position independently of the FAB). */
  private startDrag(e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startY = e.clientY;
    const base = this.pos ?? { x: window.innerWidth - 380, y: window.innerHeight - 400 };
    const onMove = (m: MouseEvent) => {
      this.pos = {
        x: Math.max(8, Math.min(window.innerWidth - 80, base.x + (m.clientX - startX))),
        y: Math.max(8, Math.min(window.innerHeight - 60, base.y + (m.clientY - startY))),
      };
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }

  /** Press-drag-or-click on the FAB: a drag moves (and persists) the button; a
   *  plain click toggles the panel. Threshold distinguishes the two. */
  private startFabPress(e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startY = e.clientY;
    const base = this.fabXY();
    let dragged = false;
    const onMove = (m: MouseEvent) => {
      const dx = m.clientX - startX;
      const dy = m.clientY - startY;
      if (!dragged && Math.hypot(dx, dy) < 4) return; // tolerance: still a click
      dragged = true;
      // Reset the panel's manual position so it re-anchors to the FAB.
      this.pos = null;
      this.fabPos = {
        x: Math.max(8, Math.min(window.innerWidth - 48, base.x + dx)),
        y: Math.max(8, Math.min(window.innerHeight - 48, base.y + dy)),
      };
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      if (dragged) {
        try { localStorage.setItem(ThinkingPopoutElement.FAB_LS, JSON.stringify(this.fabPos)); } catch { /* ignore */ }
      } else {
        this.open = !this.open; // it was a click
      }
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }

  /** Panel position: the user's manual drag (this.pos) wins; otherwise anchor it
   *  to the FAB — above it, or below if the FAB sits near the top of the screen. */
  private panelStyle(): string {
    if (this.pos) return `position:fixed; left:${this.pos.x}px; top:${this.pos.y}px;`;
    const { x, y } = this.fabXY();
    const left = Math.max(8, Math.min(window.innerWidth - 368, x + 40 - 360));
    if (y < 420) {
      // FAB near the top — open the panel below it.
      return `position:fixed; left:${left}px; top:${y + 48}px;`;
    }
    return `position:fixed; left:${left}px; bottom:${window.innerHeight - y + 8}px;`;
  }

  render() {
    const hasActivity = this.stages.size > 0;
    const { x: fabX, y: fabY } = this.fabXY();
    const panelStyle = this.panelStyle();
    return html`
      <button class="thinking-fab ${this.live ? "live" : ""}" title="AI activity / thinking — drag to move"
        style="position:fixed; left:${fabX}px; top:${fabY}px; right:auto; bottom:auto;"
        @mousedown=${(e: MouseEvent) => this.startFabPress(e)}>
        🧠${this.live ? html`<span class="thinking-dot"></span>` : ""}
      </button>
      ${this.open
        ? html`
            <div class="thinking-popout" style=${panelStyle}>
              <div class="thinking-head" @mousedown=${(e: MouseEvent) => this.startDrag(e)}>
                <span>AI activity${this.live ? " · live" : ""}</span>
                <button class="thinking-close" @click=${() => (this.open = false)} title="Close">✕</button>
              </div>
              <div class="thinking-body">
                ${!hasActivity
                  ? html`<div class="thinking-empty">No AI activity yet. Run a cleanup or summary (or re-run one) to see the prompt and response stream here.</div>`
                  : [...this.stages.entries()].map(
                      ([stage, a]) => html`
                        <div class="thinking-stage">
                          <div class="thinking-stage-title">${stageLabel(stage)}${a.done ? "" : " ⟳"}</div>
                          ${a.prompt
                            ? html`<details class="thinking-prompt"><summary>Prompt</summary><pre>${a.prompt}</pre></details>`
                            : ""}
                          <pre class="thinking-response">${a.response || "…"}</pre>
                        </div>
                      `,
                    )}
              </div>
            </div>
          `
        : ""}
    `;
  }
}
