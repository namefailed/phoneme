import { LitElement, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { subscribe, stageLabel, type DaemonEvent, type PipelineStage } from "../../services/events";

/** One AI-activity session: a single (recording, stage) run with its streamed
 *  prompt + response. Each prompt-start begins a NEW entry, so re-runs of the
 *  same stage are kept as separate log lines rather than overwriting. */
type ActivityEntry = {
  id: string;
  stage: PipelineStage;
  prompt: string;
  response: string;
  done: boolean;
  at: number;
  seq: number;
};

/**
 * A floating, resizable popout showing a COMPLETE running log of all AI
 * activity since the app opened — transcription, cleanup, and summary, for
 * every recording — with the exact prompt and the response as it streams. The
 * 🧠 button is drag-to-move (and pulses while anything is live); the panel
 * anchors to it and can be resized (size + position are remembered).
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
  /** Bumped on every activity event to force a re-render (data lives in arrays). */
  @state() private rev = 0;

  /** Complete, ordered log of every AI-activity session since launch. */
  private log: ActivityEntry[] = [];
  /** Pointer to the in-progress entry per "id|stage" so deltas update the right one. */
  private current = new Map<string, ActivityEntry>();
  private seq = 0;
  private unsub: (() => void) | null = null;

  private panelWired = false;
  private resizeObs: ResizeObserver | null = null;
  private sizeTimer: ReturnType<typeof setTimeout> | null = null;

  private static readonly FAB_LS = "phoneme.thinkingFabPos";
  private static readonly OPEN_LS = "phoneme.thinkingFabOpen";
  private static readonly SIZE_LS = "phoneme.thinkingPanelSize";
  /** Generous cap so the log is "complete" for a session without unbounded growth. */
  private static readonly MAX_ENTRIES = 200;
  /** A not-done entry older than this stops counting as "live" (guards against a
   *  stage that errored without emitting a terminal event leaving a stuck pulse). */
  private static readonly LIVE_TTL_MS = 120_000;

  private setOpen(v: boolean) {
    this.open = v;
    try { localStorage.setItem(ThinkingPopoutElement.OPEN_LS, String(v)); } catch { /* ignore */ }
  }

  async connectedCallback() {
    super.connectedCallback();
    try {
      const raw = localStorage.getItem(ThinkingPopoutElement.FAB_LS);
      if (raw) {
        const p = JSON.parse(raw);
        if (typeof p?.x === "number" && typeof p?.y === "number") this.fabPos = p;
      }
    } catch { /* ignore */ }
    try {
      this.open = localStorage.getItem(ThinkingPopoutElement.OPEN_LS) === "true";
    } catch { /* ignore */ }
    this.unsub = await subscribe((event: DaemonEvent) => {
      if (event.event !== "llm_activity") return;
      const key = `${event.id}|${event.stage}`;
      // A non-empty prompt marks the start of a new session → new log entry.
      if (event.prompt) {
        const entry: ActivityEntry = {
          id: event.id,
          stage: event.stage,
          prompt: event.prompt,
          response: event.delta ?? "",
          done: !!event.done,
          at: Date.now(),
          seq: this.seq++,
        };
        this.pushEntry(entry);
        this.current.set(key, entry);
      } else {
        let entry = this.current.get(key);
        if (!entry) {
          // Delta/done without a preceding prompt — open a new entry for it.
          entry = { id: event.id, stage: event.stage, prompt: "", response: "", done: false, at: Date.now(), seq: this.seq++ };
          this.pushEntry(entry);
          this.current.set(key, entry);
        }
        if (event.delta) entry.response += event.delta;
        if (event.done) entry.done = true;
      }
      this.rev++;
    });
  }

  private pushEntry(entry: ActivityEntry) {
    this.log.push(entry);
    if (this.log.length > ThinkingPopoutElement.MAX_ENTRIES) this.log.shift();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsub) this.unsub();
    this.resizeObs?.disconnect();
    this.resizeObs = null;
  }

  private liveCount(): number {
    const now = Date.now();
    return this.log.filter((e) => !e.done && now - e.at < ThinkingPopoutElement.LIVE_TTL_MS).length;
  }

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
      if (!dragged && Math.hypot(dx, dy) < 4) return;
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
        this.setOpen(!this.open);
      }
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }

  /** Position the panel near the FAB without ever covering it or running off the
   *  screen. It opens BELOW the button when there's room, otherwise ABOVE, using
   *  the panel's actual size (so it still fits after a resize), and clamps to the
   *  viewport so nothing is cut off at the edges. Applied imperatively
   *  (per-property) so the user's resize (inline width/height) survives
   *  re-renders. The FAB is 40px square. */
  private applyPosition(panel: HTMLElement) {
    const { x, y } = this.fabXY();
    const w = panel.offsetWidth || 380;
    const h = panel.offsetHeight || 420;
    const m = 8;
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    // Horizontal: keep the panel's right edge near the FAB (extends left); open
    // rightward only if the FAB hugs the left edge. Clamp on-screen.
    let left = x + 40 - w;
    if (left < m) left = x;
    left = Math.max(m, Math.min(left, vw - w - m));

    // Vertical: below the FAB if it fits there, else above; if neither fits
    // (very tall panel) pin to the bottom. Never overlaps the FAB unless the
    // panel is taller than the whole viewport.
    const spaceBelow = vh - (y + 40) - m;
    const spaceAbove = y - m;
    let top: number;
    if (h <= spaceBelow) top = y + 48;
    else if (h <= spaceAbove) top = y - h - 8;
    else top = vh - h - m;
    top = Math.max(m, Math.min(top, vh - h - m));

    panel.style.position = "fixed";
    panel.style.left = `${left}px`;
    panel.style.top = `${top}px`;
    panel.style.right = "auto";
    panel.style.bottom = "auto";
  }

  private restoreSize(panel: HTMLElement) {
    try {
      const raw = localStorage.getItem(ThinkingPopoutElement.SIZE_LS);
      if (raw) {
        const s = JSON.parse(raw);
        if (typeof s?.w === "number" && typeof s?.h === "number") {
          panel.style.width = `${s.w}px`;
          panel.style.height = `${s.h}px`;
        }
      }
    } catch { /* ignore */ }
  }

  private observeSize(panel: HTMLElement) {
    this.resizeObs = new ResizeObserver(() => {
      if (this.sizeTimer) clearTimeout(this.sizeTimer);
      // Persist only (no setState) so this never feeds back into a render loop.
      this.sizeTimer = setTimeout(() => {
        try {
          localStorage.setItem(
            ThinkingPopoutElement.SIZE_LS,
            JSON.stringify({ w: panel.offsetWidth, h: panel.offsetHeight }),
          );
        } catch { /* ignore */ }
      }, 300);
    });
    this.resizeObs.observe(panel);
  }

  updated() {
    const panel = this.renderRoot.querySelector<HTMLElement>(".thinking-popout");
    if (panel) {
      if (!this.panelWired) {
        this.panelWired = true;
        this.restoreSize(panel);
        this.observeSize(panel);
      }
      this.applyPosition(panel);
    } else if (this.panelWired) {
      this.panelWired = false;
      this.resizeObs?.disconnect();
      this.resizeObs = null;
    }
  }

  private renderEntry(e: ActivityEntry) {
    const time = new Date(e.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
    const mine = e.id === this.recordingId;
    return html`
      <div class="thinking-entry ${e.done ? "" : "live"}">
        <div class="thinking-entry-head">
          <span class="thinking-entry-stage">${stageLabel(e.stage)}${e.done ? "" : " ⟳"}</span>
          <span class="thinking-entry-meta">${mine ? "● " : ""}#${e.id.slice(0, 6)} · ${time}</span>
        </div>
        ${e.prompt
          ? html`<details class="thinking-prompt"><summary>Prompt</summary><pre>${e.prompt}</pre></details>`
          : ""}
        <pre class="thinking-response">${e.response || "…"}</pre>
      </div>
    `;
  }

  render() {
    void this.rev; // re-render on activity
    const { x: fabX, y: fabY } = this.fabXY();
    const live = this.liveCount();
    // Newest first.
    const entries = [...this.log].reverse();
    return html`
      <button class="thinking-fab ${live ? "live" : ""}" title="AI activity — drag to move, click to open"
        style="position:fixed; left:${fabX}px; top:${fabY}px; right:auto; bottom:auto;"
        @mousedown=${(e: MouseEvent) => this.startFabPress(e)}>
        🧠${live ? html`<span class="thinking-dot"></span>` : ""}
      </button>
      ${this.open
        ? html`
            <div class="thinking-popout">
              <div class="thinking-head">
                <span>AI activity${live ? ` · ${live} live` : ""}${this.log.length ? ` · ${this.log.length}` : ""}</span>
                <button class="thinking-close" @click=${() => this.setOpen(false)} title="Close">✕</button>
              </div>
              <div class="thinking-body">
                ${entries.length === 0
                  ? html`<div class="thinking-empty">No AI activity yet. Transcribe, clean up, or summarize a recording (or re-run one) and the prompt + response will stream here. Everything since you opened the app is kept in this list.</div>`
                  : entries.map((e) => this.renderEntry(e))}
              </div>
            </div>
          `
        : ""}
    `;
  }
}
