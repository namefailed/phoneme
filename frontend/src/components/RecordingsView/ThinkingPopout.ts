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
  /** Floating position; null = default (anchored bottom-right). */
  @state() private pos: { x: number; y: number } | null = null;

  private unsub: (() => void) | null = null;

  async connectedCallback() {
    super.connectedCallback();
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

  private startDrag(e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startY = e.clientY;
    const base = this.pos ?? { x: window.innerWidth - 380, y: window.innerHeight - 360 };
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

  render() {
    const hasActivity = this.stages.size > 0;
    const panelStyle = this.pos
      ? `position:fixed; left:${this.pos.x}px; top:${this.pos.y}px;`
      : `position:fixed; right:20px; bottom:20px;`;
    return html`
      <button class="thinking-fab ${this.live ? "live" : ""}" title="AI activity / thinking"
        @click=${() => (this.open = !this.open)}>
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
