import { LitElement, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { subscribe, stageLabel, type DaemonEvent, type PipelineStage } from "../../services/events";

type StageActivity = { prompt: string; response: string; done: boolean };

/**
 * A draggable floating popout showing live AI "thinking": the exact prompt sent
 * to the LLM and the response as it streams (token-by-token for Ollama;
 * whole-response for non-streaming providers), per stage (cleanup / summary).
 *
 * It captures activity for EVERY recording as it streams (keyed by id), and
 * displays the selected recording's activity — or, if the selection has none,
 * whatever is currently/most-recently streaming. So clicking the 🧠 button
 * always shows the latest AI activity, whether or not you pre-selected the row.
 * The button is drag-to-move; the panel anchors to it.
 */
@customElement("ph-thinking-popout")
export class ThinkingPopoutElement extends LitElement {
  protected createRenderRoot() {
    return this; // light DOM for global CSS / theme vars
  }

  @property({ type: String }) recordingId = "";

  @state() private open = false;
  /** FAB (button) position, draggable + persisted; null = default bottom-right. */
  @state() private fabPos: { x: number; y: number } | null = null;
  /** Bumped on every activity event to force a re-render (the data lives in a Map). */
  @state() private rev = 0;

  /** AI activity per recording id → per stage. Bounded to the last few recordings. */
  private byRecording = new Map<string, Map<PipelineStage, StageActivity>>();
  /** Id of the recording that most recently streamed (for the no-selection case). */
  private activeId = "";
  private unsub: (() => void) | null = null;
  private static readonly FAB_LS = "phoneme.thinkingFabPos";
  private static readonly MAX_TRACKED = 6;

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
      let rec = this.byRecording.get(event.id);
      if (!rec) {
        rec = new Map();
        this.byRecording.set(event.id, rec);
        // Bound memory: drop the oldest tracked recording.
        if (this.byRecording.size > ThinkingPopoutElement.MAX_TRACKED) {
          const oldest = this.byRecording.keys().next().value;
          if (oldest !== undefined) this.byRecording.delete(oldest);
        }
      }
      const cur = rec.get(event.stage) ?? { prompt: "", response: "", done: false };
      if (event.prompt) cur.prompt = event.prompt;
      if (event.delta) cur.response += event.delta;
      if (event.done) cur.done = true;
      rec.set(event.stage, cur);
      if (!event.done) this.activeId = event.id; // currently streaming
      this.rev++;
    });
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsub) this.unsub();
  }

  /** Which recording's activity to show: the selected one if it has activity,
   *  otherwise whatever is currently/most-recently streaming. */
  private displayId(): string {
    if (this.recordingId && this.byRecording.has(this.recordingId)) return this.recordingId;
    return this.activeId;
  }

  private stagesFor(id: string): [PipelineStage, StageActivity][] {
    const rec = this.byRecording.get(id);
    return rec ? [...rec.entries()] : [];
  }

  private isLive(id: string): boolean {
    const rec = this.byRecording.get(id);
    return !!rec && [...rec.values()].some((s) => !s.done);
  }

  /** Current FAB top-left (resolved default = bottom-right with a 20px margin). */
  private fabXY(): { x: number; y: number } {
    return this.fabPos ?? { x: window.innerWidth - 60, y: window.innerHeight - 60 };
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

  /** Panel position: anchored to the FAB — above it, or below if the FAB sits
   *  near the top of the screen; opening to the left of the button by default,
   *  or to the right when the button hugs the left edge. Always tracks it. */
  private panelStyle(): string {
    const { x, y } = this.fabXY();
    const PANEL_W = 360;
    // Prefer the panel's right edge near the FAB (opens leftward). If the button
    // is too close to the left edge for that to fit, open rightward instead.
    const leftAnchored = x + 40 - PANEL_W;
    const left = leftAnchored >= 8
      ? Math.min(leftAnchored, window.innerWidth - PANEL_W - 8)
      : Math.max(8, Math.min(x, window.innerWidth - PANEL_W - 8));
    if (y < 420) return `position:fixed; left:${left}px; top:${y + 48}px;`;
    return `position:fixed; left:${left}px; bottom:${window.innerHeight - y + 8}px;`;
  }

  render() {
    void this.rev; // re-render on activity
    const { x: fabX, y: fabY } = this.fabXY();
    const id = this.displayId();
    const stages = id ? this.stagesFor(id) : [];
    const live = id ? this.isLive(id) : false;
    return html`
      <button class="thinking-fab ${live ? "live" : ""}" title="AI activity / thinking — drag to move"
        style="position:fixed; left:${fabX}px; top:${fabY}px; right:auto; bottom:auto;"
        @mousedown=${(e: MouseEvent) => this.startFabPress(e)}>
        🧠${live ? html`<span class="thinking-dot"></span>` : ""}
      </button>
      ${this.open
        ? html`
            <div class="thinking-popout" style=${this.panelStyle()}>
              <div class="thinking-head">
                <span>AI activity${live ? " · live" : ""}</span>
                <button class="thinking-close" @click=${() => (this.open = false)} title="Close">✕</button>
              </div>
              <div class="thinking-body">
                ${stages.length === 0
                  ? html`<div class="thinking-empty">No AI activity yet. Run a cleanup or summary (or re-run one), or record with auto-cleanup/summary on, and the prompt + response will stream here.</div>`
                  : stages.map(
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
