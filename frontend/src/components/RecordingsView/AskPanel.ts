import { LitElement, html, type TemplateResult } from "lit";
import { customElement, state, query } from "lit/decorators.js";
import { subscribe, type AskActivitySource, type DaemonEvent } from "../../services/events";
import { ask } from "../../services/ipc";
import { closeModalOverlay } from "../../utils/modalAnim";

/**
 * "Ask my archive" — a local-RAG chat modal that answers a question grounded in
 * the user's own transcripts, with citations.
 *
 * Opened by the `phoneme:open-ask` window event (the header's 💬 button
 * dispatches it). On submit it mints a `requestId` (so it can filter the shared
 * daemon-event stream with no race), subscribes to `ask_activity`, then invokes
 * `ask`. The daemon streams the citation sources first, then the answer deltas,
 * then a terminal `done`. The answer's inline `[n]` markers are rendered as
 * clickable chips that open `sources[n-1]` in the detail pane (via
 * `phoneme:select-recording`); an out-of-range marker the model invents stays
 * plain text. Esc / overlay-click / ✕ close it, honoring `--ui-motion`.
 *
 * Retrieval + generation are entirely the daemon's (see `bin/phoneme-daemon`'s
 * `ask` module); this component only drives the request and renders the stream.
 *
 * NEEDS-NATIVE-VERIFY: the headless preview browser can't render the
 * invoke-driven Tauri app, so the live stream + citation chips must be checked
 * in the native window.
 */
@customElement("ph-ask-panel")
export class AskPanelElement extends LitElement {
  protected createRenderRoot() {
    return this; // light DOM for global CSS / theme vars
  }

  @state() private openState = false;
  @state() private query = "";
  @state() private busy = false;
  @state() private sources: AskActivitySource[] = [];
  @state() private answer = "";
  @state() private error = "";
  /** True once the daemon shipped its sources event (even if empty) — so the UI
   *  can tell "nothing matched" apart from "still retrieving". */
  @state() private gotSources = false;

  @query("#ask-input") private input?: HTMLTextAreaElement;

  /** The request this panel is currently streaming; events for any other id are
   *  ignored (the bus is shared with every other Ask). Empty = idle. */
  private requestId = "";
  private unsub: (() => void) | null = null;

  private onOpen = () => this.setOpen(true);
  private keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape" && this.openState) {
      e.stopPropagation();
      this.close();
    }
  };

  async connectedCallback() {
    super.connectedCallback();
    window.addEventListener("phoneme:open-ask", this.onOpen);
    document.addEventListener("keydown", this.keyHandler);
    const unsub = await subscribe((event: DaemonEvent) => this.onEvent(event));
    if (!this.isConnected) unsub();
    else this.unsub = unsub;
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("phoneme:open-ask", this.onOpen);
    document.removeEventListener("keydown", this.keyHandler);
    if (this.unsub) this.unsub();
  }

  private setOpen(v: boolean) {
    this.openState = v;
    this.toggleAttribute("data-open", v);
    if (v) {
      // Focus the input once it's in the DOM.
      this.updateComplete.then(() => this.input?.focus());
    }
  }

  private close() {
    const overlay = this.querySelector<HTMLElement>(".modal-overlay");
    const done = () => {
      this.setOpen(false);
      // Leave the last answer in place so re-opening shows it; only the live
      // request id is cleared so late events for it stop mattering.
      this.requestId = "";
      this.busy = false;
    };
    if (overlay) closeModalOverlay(overlay, done);
    else done();
  }

  private onEvent(event: DaemonEvent) {
    if (event.event !== "ask_activity") return;
    if (event.request_id !== this.requestId || !this.requestId) return;
    if (event.sources.length > 0 || (!this.gotSources && !event.delta && !event.done)) {
      // The first event carries the citation sources (possibly empty).
      this.sources = event.sources;
      this.gotSources = true;
    }
    if (event.delta) this.answer += event.delta;
    if (event.done) {
      this.busy = false;
      if (event.error) this.error = event.error;
    }
    this.requestUpdate();
  }

  private submit() {
    const q = this.query.trim();
    if (!q || this.busy) return;
    // Reset per-question state and mint a fresh correlation id BEFORE invoking,
    // so every streamed event is matched to this request.
    this.requestId = crypto.randomUUID();
    this.answer = "";
    this.sources = [];
    this.error = "";
    this.gotSources = false;
    this.busy = true;
    void ask(this.requestId, q).catch((e: unknown) => {
      // A synchronous reject is an up-front failure (no embedder / no provider).
      this.busy = false;
      this.error = errorMessage(e);
      this.requestUpdate();
    });
  }

  private onKeydown(e: KeyboardEvent) {
    // Enter submits; Shift+Enter inserts a newline (multi-line questions).
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      this.submit();
    }
  }

  /** Open the recording a citation points at, in the detail pane, then close. */
  private openSource(recordingId: string) {
    window.dispatchEvent(
      new CustomEvent("phoneme:select-recording", { detail: { id: recordingId } }),
    );
    this.close();
  }

  /** Render the answer text with inline `[n]` markers turned into clickable
   *  chips that open the matching source. An `[n]` with no matching source
   *  (a marker the model invented) is left as plain text. */
  private renderAnswer(): TemplateResult[] {
    const parts: TemplateResult[] = [];
    const re = /\[(\d+)\]/g;
    let last = 0;
    let m: RegExpExecArray | null;
    while ((m = re.exec(this.answer)) !== null) {
      if (m.index > last) {
        parts.push(html`${this.answer.slice(last, m.index)}`);
      }
      const n = Number(m[1]);
      const src = this.sources.find((s) => s.n === n);
      if (src) {
        parts.push(html`<button
          class="ask-cite"
          title=${src.label}
          @click=${() => this.openSource(src.recording_id)}
        >[${n}]</button>`);
      } else {
        parts.push(html`${m[0]}`);
      }
      last = m.index + m[0].length;
    }
    if (last < this.answer.length) parts.push(html`${this.answer.slice(last)}`);
    return parts;
  }

  private handleOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) this.close();
  }

  render() {
    if (!this.openState) return html``;
    const nothingMatched = this.gotSources && this.sources.length === 0;
    return html`
      <div class="modal-overlay" @click=${this.handleOverlayClick}>
        <div class="modal-dialog ask-dialog" role="dialog" aria-modal="true" aria-labelledby="ask-title">
          <div class="modal-header">
            <span class="modal-icon" aria-hidden="true">💬</span>
            <h3 class="modal-title" id="ask-title">Ask my archive</h3>
            <button class="thinking-close ask-close" @click=${() => this.close()} title="Close (Esc)">✕</button>
          </div>

          <div class="ask-input-row">
            <textarea
              id="ask-input"
              class="ask-input"
              rows="2"
              placeholder="Ask a question about your recordings… (Enter to send, Shift+Enter for a new line)"
              .value=${this.query}
              ?disabled=${this.busy}
              @input=${(e: Event) => (this.query = (e.target as HTMLTextAreaElement).value)}
              @keydown=${(e: KeyboardEvent) => this.onKeydown(e)}
            ></textarea>
            <button class="modal-btn modal-btn-primary ask-send" ?disabled=${this.busy || !this.query.trim()} @click=${() => this.submit()}>
              ${this.busy ? "Asking…" : "Ask"}
            </button>
          </div>

          ${this.error
            ? html`<p class="ask-error" role="alert">${this.error}</p>`
            : ""}

          ${this.sources.length > 0
            ? html`
                <div class="ask-sources">
                  <div class="ask-sources-head">Sources</div>
                  <ol class="ask-sources-list">
                    ${this.sources.map(
                      (s) => html`
                        <li class="ask-source">
                          <button class="ask-source-link" @click=${() => this.openSource(s.recording_id)} title=${s.snippet}>
                            <span class="ask-source-n">[${s.n}]</span>
                            <span class="ask-source-label">${s.label}</span>
                            <span class="ask-source-rel">${Math.round(Math.max(0, Math.min(1, s.relevance)) * 100)}%</span>
                          </button>
                        </li>
                      `,
                    )}
                  </ol>
                </div>
              `
            : ""}

          <div class="ask-answer">
            ${this.answer
              ? html`<div class="ask-answer-text">${this.renderAnswer()}</div>`
              : nothingMatched
                ? ""
                : this.busy
                  ? html`<div class="ask-thinking"><span class="thinking-spin" aria-hidden="true"></span> Searching your recordings…</div>`
                  : html`<div class="ask-hint">Answers are grounded only in your own transcripts, with a citation for every claim.</div>`}
          </div>
        </div>
      </div>
    `;
  }
}

/** Best-effort message from an unknown thrown/rejected value. */
function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object") {
    const o = e as { message?: unknown };
    if (typeof o.message === "string") return o.message;
  }
  return "Ask failed";
}
