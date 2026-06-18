import { LitElement, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { repeat } from "lit/directives/repeat.js";
import { subscribe, stageLabel, type DaemonEvent, type PipelineStage } from "../../services/events";
import { listAiActivity } from "../../services/ipc";
import { showToast } from "../../utils/toast";

/** A right-pointing chevron that rotates to point down when its <details> is
 *  open (see `.thinking-chevron` CSS) — replaces the native disclosure triangle. */
const CHEVRON = html`<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor"
  stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>`;
/** A small clipboard glyph for the per-section copy buttons. */
const COPY_ICON = html`<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor"
  stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>`;

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
 * A floating, resizable popout showing a running log of AI activity — the live
 * transcription/cleanup/summary stream (exact prompt + streamed response) for
 * every recording, PLUS the recent persisted sessions loaded on open so the log
 * survives app restarts (`list_ai_activity`). The 🧠 button is drag-to-move (and
 * pulses while anything is live); the panel anchors to it and can be resized
 * (size + position are remembered).
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
  /** Recompute the anchored default when the sidebar toggles/resizes or the
   *  window resizes (only matters while no custom position is set). */
  private onLayoutChange = () => { if (!this.fabPos) this.requestUpdate(); };

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
    // Reflect open-ness onto the host so the global keyboard layer can detect an
    // open panel (`ph-thinking-popout[data-open]`) and close it on Escape.
    this.toggleAttribute("data-open", v);
    try { localStorage.setItem(ThinkingPopoutElement.OPEN_LS, String(v)); } catch { /* ignore */ }
  }

  /** Toggle the panel from the keyboard (the `g A` chord dispatches this). */
  private onToggleActivity = () => this.setOpen(!this.open);

  async connectedCallback() {
    super.connectedCallback();
    window.addEventListener("phoneme:sidebar-changed", this.onLayoutChange);
    window.addEventListener("resize", this.onLayoutChange);
    window.addEventListener("phoneme:toggle-ai-activity", this.onToggleActivity);
    try {
      const raw = localStorage.getItem(ThinkingPopoutElement.FAB_LS);
      if (raw) {
        const p = JSON.parse(raw);
        if (typeof p?.x === "number" && typeof p?.y === "number") this.fabPos = p;
      }
    } catch { /* ignore */ }
    try {
      this.open = localStorage.getItem(ThinkingPopoutElement.OPEN_LS) === "true";
      this.toggleAttribute("data-open", this.open);
    } catch { /* ignore */ }
    try {
      const raw = localStorage.getItem(ThinkingPopoutElement.GEOM_LS);
      if (raw) {
        const g = JSON.parse(raw);
        if (["left", "top", "width", "height"].every((k) => typeof g?.[k] === "number")) this.geom = g;
      }
    } catch { /* ignore */ }
    // Seed the log with persisted history so the panel isn't empty after a
    // restart — the live stream below only carries what happens from now on.
    await this.loadHistory();
    const unsub = await subscribe((event: DaemonEvent) => {
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
    // If the element disconnected while loadHistory/subscribe were awaiting,
    // disconnectedCallback already ran with this.unsub null — tear the late
    // listener down now rather than leaking it.
    if (!this.isConnected) unsub();
    else this.unsub = unsub;
  }

  private pushEntry(entry: ActivityEntry) {
    this.log.push(entry);
    if (this.log.length > ThinkingPopoutElement.MAX_ENTRIES) this.log.shift();
  }

  /** Seed the log from the durable AI-activity store so the panel isn't empty
   *  after an app restart — the live event stream only carries what runs from
   *  now on. Persisted sessions are completed (done), so they never count as
   *  "live". Best-effort: if the daemon isn't reachable yet, stay live-only. */
  private async loadHistory() {
    try {
      const rows = await listAiActivity(undefined, ThinkingPopoutElement.MAX_ENTRIES);
      // The store returns newest-first; insert oldest-first so the log's order
      // stays chronological (render() reverses it to show newest at the top).
      for (const r of rows.reverse()) {
        this.pushEntry({
          id: r.recording_id,
          stage: r.stage as PipelineStage,
          prompt: r.prompt,
          response: r.response,
          done: true,
          at: Date.parse(r.created_at) || Date.now(),
          seq: this.seq++,
        });
      }
      this.rev++;
    } catch {
      /* daemon not ready / no history — live-only is fine */
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("phoneme:sidebar-changed", this.onLayoutChange);
    window.removeEventListener("resize", this.onLayoutChange);
    window.removeEventListener("phoneme:toggle-ai-activity", this.onToggleActivity);
    if (this.unsub) this.unsub();
  }

  private liveCount(): number {
    const now = Date.now();
    return this.log.filter((e) => !e.done && now - e.at < ThinkingPopoutElement.LIVE_TTL_MS).length;
  }

  /** Default anchored position: bottom, just OUTSIDE the sidebar's right edge
   *  (beside it, in the list area) — or in the bottom-left corner when the
   *  sidebar is hidden. Reads the live sidebar layout so it tracks the drawer. */
  private defaultFabXY(): { x: number; y: number } {
    // Sit just outside the sidebar's right edge / the splitter (8px clear), at
    // the bottom — i.e. beside the bar, not on top of it. When the sidebar is
    // hidden, tuck into the bottom-left corner instead.
    const sb = document.querySelector<HTMLElement>("ph-sidebar");
    const right = sb ? sb.getBoundingClientRect().right : 0;
    const x = right > 24 ? Math.min(window.innerWidth - 48, right + 8) : 12;
    return { x, y: window.innerHeight - 56 };
  }

  private fabXY(): { x: number; y: number } {
    // No custom position → the sidebar-anchored default (recomputed each render
    // so it follows the drawer). Otherwise re-clamp the saved position to the
    // CURRENT viewport so a smaller window can't strand it off-screen.
    if (!this.fabPos) return this.defaultFabXY();
    return {
      x: Math.max(8, Math.min(window.innerWidth - 48, this.fabPos.x)),
      y: Math.max(8, Math.min(window.innerHeight - 48, this.fabPos.y)),
    };
  }

  /** Press-drag-or-click on the FAB: a drag moves (and persists) the button; a
   *  plain click toggles the panel. Threshold distinguishes the two. */
  private startFabPress(e: MouseEvent) {
    // Ctrl+Shift+click resets to the default sidebar-anchored position/behaviour.
    if (e.ctrlKey && e.shiftKey) {
      e.preventDefault();
      e.stopPropagation();
      this.fabPos = null;
      try { localStorage.removeItem(ThinkingPopoutElement.FAB_LS); } catch { /* ignore */ }
      this.requestUpdate();
      return;
    }
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

  /** Default placement (used until the user resizes and a geometry is saved):
   *  the panel pops out DIAGONALLY from the button toward the side with the most
   *  room — for the default bottom-right button, up and to the left — so the
   *  button sits just off the panel's corner and is never covered. Clamped to the
   *  viewport so nothing is cut off. Applied per-property so a later resize
   *  survives re-renders. The FAB is 40px square. */
  private applyPosition(panel: HTMLElement) {
    const { x, y } = this.fabXY(); // the FAB's CURRENT top-left (recomputed each
    // render, so this re-anchors when the sidebar toggle moves the default button)
    const fab = 40; // the FAB is 40px square
    const r = fab / 2;
    const w = panel.offsetWidth || 560;
    const h = panel.offsetHeight || 600;
    const m = 8; // viewport margin
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const cx = x + r;
    const cy = y + r;

    // Unfold the panel DIAGONALLY off the FAB with a gap — never overlapping it.
    // Its near corner sits just BEYOND the button's edge along the diagonal, so the
    // panel floats off the button at an angle (the button stays fully visible at
    // the origin). Grows toward whichever side has room — default button
    // (bottom-left of the list area) → panel up-and-right. Re-derived from the live
    // FAB position, so closing the sidebar (which slides the default button)
    // re-anchors the open panel.
    const openLeft = cx > vw / 2; // button on the right half → grow leftward
    const openUp = cy > vh / 2; // button on the bottom half → grow upward
    const gap = 10; // clear space between the button's edge and the panel's corner
    const off = r + gap; // distance from the button CENTRE to the panel's corner
    const cornerX = openLeft ? cx - off : cx + off;
    const cornerY = openUp ? cy - off : cy + off;
    let left = openLeft ? cornerX - w : cornerX;
    let top = openUp ? cornerY - h : cornerY;

    left = Math.max(m, Math.min(left, vw - w - m));
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

  /** Drag the whole panel by its title bar. Sets a geometry (so it takes over
   *  from FAB-anchoring) and persists it, mirroring the resize handles. Ignores
   *  drags that start on the close button. */
  private startHeadDrag(e: MouseEvent) {
    if ((e.target as HTMLElement).closest(".thinking-close")) return;
    // Ctrl+Shift+click the title → reset the panel to its default size + position
    // (mirrors the FAB's Ctrl+Shift+click reset). Drop the saved geometry and the
    // inline width/height so the CSS default (560×600) and FAB-anchored placement
    // both come back.
    if (e.ctrlKey && e.shiftKey) {
      e.preventDefault();
      e.stopPropagation();
      this.geom = null;
      try { localStorage.removeItem(ThinkingPopoutElement.GEOM_LS); } catch { /* ignore */ }
      const reset = this.renderRoot.querySelector<HTMLElement>(".thinking-popout");
      if (reset) { reset.style.width = ""; reset.style.height = ""; }
      this.requestUpdate();
      showToast("AI Activity panel reset to default", "success");
      return;
    }
    e.preventDefault();
    const panel = this.renderRoot.querySelector<HTMLElement>(".thinking-popout");
    if (!panel) return;
    const r = panel.getBoundingClientRect();
    const startX = e.clientX, startY = e.clientY;
    const sl = r.left, st = r.top, w = r.width, h = r.height;
    const onMove = (mv: MouseEvent) => {
      const left = Math.max(8, Math.min(sl + (mv.clientX - startX), window.innerWidth - w - 8));
      const top = Math.max(8, Math.min(st + (mv.clientY - startY), window.innerHeight - h - 8));
      this.geom = { left, top, width: w, height: h };
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

  /** Copy a section's text without toggling its <details> (preventDefault stops
   *  the summary click from opening/closing the fold). */
  private async copy(ev: MouseEvent, text: string, what: string) {
    ev.preventDefault();
    ev.stopPropagation();
    try {
      await navigator.clipboard.writeText(text);
      showToast(`Copied ${what}`, "success");
    } catch {
      showToast(`Couldn't copy ${what}`, "error");
    }
  }

  /** One collapsible section (Prompt / Response). The literal `open` attribute is
   *  static, so lit never re-applies it on the streaming re-renders — once the
   *  user folds/unfolds a section it stays that way. `kind` drives extra styling
   *  (the response gets a scroll cap). */
  private foldSection(label: string, kind: string, body: string, defaultOpen: boolean) {
    const head = html`
      <summary class="thinking-fold-sum">
        <span class="thinking-chevron" aria-hidden="true">${CHEVRON}</span>
        <span class="thinking-fold-label">${label}</span>
        <button class="thinking-copy" title="Copy ${label.toLowerCase()}"
          @click=${(ev: MouseEvent) => this.copy(ev, body, label.toLowerCase())}>${COPY_ICON}</button>
      </summary>
      <pre class="thinking-pre thinking-pre--${kind}">${body}</pre>`;
    return defaultOpen
      ? html`<details class="thinking-fold" open>${head}</details>`
      : html`<details class="thinking-fold">${head}</details>`;
  }

  private renderEntry(e: ActivityEntry) {
    const time = new Date(e.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
    const mine = e.id === this.recordingId;
    return html`
      <div class="thinking-entry ${e.done ? "" : "live"}">
        <div class="thinking-entry-head">
          <span class="thinking-entry-stage">
            ${e.done ? "" : html`<span class="thinking-spin" aria-hidden="true"></span>`}
            ${stageLabel(e.stage)}
          </span>
          <span class="thinking-entry-meta">
            ${mine ? html`<span class="thinking-mine-dot" title="This recording"></span>` : ""}<span class="thinking-id">#${e.id.slice(0, 6)}</span> · ${time}
          </span>
        </div>
        ${e.prompt ? this.foldSection("Prompt", "prompt", e.prompt, false) : ""}
        ${this.foldSection("Response", "response", e.response || "…", true)}
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
              <div class="thinking-head" title="Drag to move · Ctrl+Shift+click to reset size & position" @mousedown=${(e: MouseEvent) => this.startHeadDrag(e)}>
                <span class="thinking-title">
                  <span class="thinking-title-icon" aria-hidden="true">🧠</span>
                  <span class="thinking-title-text">AI Activity</span>
                  ${live
                    ? html`<span class="thinking-title-live" title="${live} stage${live === 1 ? "" : "s"} running now"><span class="thinking-title-live-dot"></span>${live} live</span>`
                    : ""}
                  ${this.log.length
                    ? html`<span class="thinking-title-count" title="Recent AI sessions (kept across restarts)">${this.log.length} ${this.log.length === 1 ? "entry" : "entries"}</span>`
                    : ""}
                </span>
                <button class="thinking-close" @click=${() => this.setOpen(false)} title="Close">✕</button>
              </div>
              <div class="thinking-body">
                ${entries.length === 0
                  ? html`<div class="thinking-empty">No AI activity yet. Clean up or summarize a recording (or re-run one) and the prompt + response will stream here. Recent sessions are kept across restarts.</div>`
                  : repeat(entries, (e) => e.seq, (e) => this.renderEntry(e))}
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
