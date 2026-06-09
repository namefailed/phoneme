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

  /** User-set panel geometry from edge/corner resizing; null = auto-anchored to
   *  the FAB at the default size. Once the user resizes, this takes over. */
  private geom: { left: number; top: number; width: number; height: number } | null = null;

  private static readonly FAB_LS = "phoneme.thinkingFabPos";
  private static readonly OPEN_LS = "phoneme.thinkingFabOpen";
  private static readonly GEOM_LS = "phoneme.thinkingPanelGeom";
  private static readonly MIN_W = 300;
  private static readonly MIN_H = 220;
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
    try {
      const raw = localStorage.getItem(ThinkingPopoutElement.GEOM_LS);
      if (raw) {
        const g = JSON.parse(raw);
        if (["left", "top", "width", "height"].every((k) => typeof g?.[k] === "number")) this.geom = g;
      }
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

  /** Apply the user's saved geometry (from edge/corner resizing), clamped to the
   *  viewport so it's always on-screen and reachable. */
  private applyGeom(panel: HTMLElement) {
    if (!this.geom) return;
    const m = 8;
    const width = Math.max(ThinkingPopoutElement.MIN_W, Math.min(this.geom.width, window.innerWidth * 0.92));
    const height = Math.max(ThinkingPopoutElement.MIN_H, Math.min(this.geom.height, window.innerHeight * 0.85));
    const left = Math.max(m, Math.min(this.geom.left, window.innerWidth - width - m));
    const top = Math.max(m, Math.min(this.geom.top, window.innerHeight - height - m));
    panel.style.position = "fixed";
    panel.style.left = `${left}px`;
    panel.style.top = `${top}px`;
    panel.style.width = `${width}px`;
    panel.style.height = `${height}px`;
    panel.style.right = "auto";
    panel.style.bottom = "auto";
  }

  /** Drag a resize handle. `dir` contains any of n/s/e/w; corners combine two.
   *  Resizing from a top/left edge moves that edge while keeping the opposite
   *  edge fixed. The resulting geometry takes over from FAB-anchoring. */
  private startResize(e: MouseEvent, dir: string) {
    e.preventDefault();
    e.stopPropagation();
    const panel = this.renderRoot.querySelector<HTMLElement>(".thinking-popout");
    if (!panel) return;
    const r = panel.getBoundingClientRect();
    const sl = r.left, st = r.top, sw = r.width, sh = r.height;
    const mx = e.clientX, my = e.clientY;
    const minW = ThinkingPopoutElement.MIN_W, minH = ThinkingPopoutElement.MIN_H;
    const maxW = window.innerWidth * 0.92, maxH = window.innerHeight * 0.85;
    const onMove = (mv: MouseEvent) => {
      const dx = mv.clientX - mx, dy = mv.clientY - my;
      let left = sl, top = st, width = sw, height = sh;
      if (dir.includes("e")) width = sw + dx;
      if (dir.includes("w")) width = sw - dx;
      if (dir.includes("s")) height = sh + dy;
      if (dir.includes("n")) height = sh - dy;
      width = Math.max(minW, Math.min(maxW, width));
      height = Math.max(minH, Math.min(maxH, height));
      if (dir.includes("w")) left = sl + (sw - width);   // keep the right edge fixed
      if (dir.includes("n")) top = st + (sh - height);    // keep the bottom edge fixed
      left = Math.max(8, Math.min(left, window.innerWidth - width - 8));
      top = Math.max(8, Math.min(top, window.innerHeight - height - 8));
      this.geom = { left, top, width, height };
      this.applyGeom(panel);
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      try { localStorage.setItem(ThinkingPopoutElement.GEOM_LS, JSON.stringify(this.geom)); } catch { /* ignore */ }
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }

  updated() {
    const panel = this.renderRoot.querySelector<HTMLElement>(".thinking-popout");
    if (!panel) return;
    // Once the user has resized, honor their geometry; otherwise anchor to the FAB.
    if (this.geom) this.applyGeom(panel);
    else this.applyPosition(panel);
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
              <span class="thinking-rz thinking-rz-n" @mousedown=${(e: MouseEvent) => this.startResize(e, "n")}></span>
              <span class="thinking-rz thinking-rz-s" @mousedown=${(e: MouseEvent) => this.startResize(e, "s")}></span>
              <span class="thinking-rz thinking-rz-e" @mousedown=${(e: MouseEvent) => this.startResize(e, "e")}></span>
              <span class="thinking-rz thinking-rz-w" @mousedown=${(e: MouseEvent) => this.startResize(e, "w")}></span>
              <span class="thinking-rz thinking-rz-ne" @mousedown=${(e: MouseEvent) => this.startResize(e, "ne")}></span>
              <span class="thinking-rz thinking-rz-nw" @mousedown=${(e: MouseEvent) => this.startResize(e, "nw")}></span>
              <span class="thinking-rz thinking-rz-se" @mousedown=${(e: MouseEvent) => this.startResize(e, "se")}></span>
              <span class="thinking-rz thinking-rz-sw" @mousedown=${(e: MouseEvent) => this.startResize(e, "sw")}></span>
            </div>
          `
        : ""}
    `;
  }
}
