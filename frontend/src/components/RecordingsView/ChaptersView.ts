/**
 * The per-recording topic timeline: the recording's auto-chapters rendered as a
 * clickable, time-coded list (the "Chapters" peek in the detail pane).
 *
 * A hybrid of {@link TimelineView} (clickable, time-coded rows that seek the host
 * waveform, with a playhead-follow highlight) and {@link EntityChips} (a Lit
 * element that loads its own data and live-refreshes on the daemon event, with a
 * generate-on-demand affordance). Each row is one chapter: its start clock, title,
 * and an optional one-line summary; clicking a row seeks the host pane's waveform
 * to that chapter's start.
 *
 * Chapters are an LLM artifact — empty until the auto-chapter step runs (in the
 * pipeline, or on demand here). An empty list is a normal state and renders as a
 * "Generate chapters" affordance, not an error. A recording with no transcript
 * timing can't be chaptered; the daemon returns that as a clean no-op, so the
 * Generate button simply leaves the list empty.
 */
import { LitElement, html, type PropertyValues } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { getChapters, suggestChapters, type Chapter } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { fmtClock } from "../../utils/format";
import { showToast } from "../../utils/toast";
import { errText } from "../../utils/error";

/** Index of the chapter containing `ms` (or the last one started before it); -1
 *  before the first chapter. Chapters are in chronological order. Extracted +
 *  exported so it can be unit-tested without a DOM (mirrors `activeGroupIndex`). */
export function activeChapterIndex(chapters: Chapter[], ms: number): number {
  let active = -1;
  for (let i = 0; i < chapters.length; i++) {
    if (chapters[i].start_ms <= ms) active = i;
    else break;
  }
  return active;
}

@customElement("ph-chapters-view")
export class ChaptersViewElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM, to inherit the global timeline/chip styles.
  }

  @property({ type: String }) recordingId = "";
  /** Host waveform seek callback, set by the imperative wrapper. */
  onSeek: (seconds: number) => void = () => {};

  @state() private chapters: Chapter[] = [];
  /** True while an on-demand ✨ Generate run is in flight. */
  @state() private generating = false;
  /** True until the first load resolves, so the empty state doesn't flash before
   *  the data arrives. */
  @state() private loading = true;
  /** The chapter under the playhead, highlighted as playback advances. -1 = none. */
  @state() private activeIdx = -1;
  private unsubEvents: (() => void) | null = null;

  connectedCallback() {
    super.connectedCallback();
    if (this.recordingId) void this.load();
    void subscribe((e: DaemonEvent) => {
      if (e.event === "chapters_updated" && e.id === this.recordingId) {
        this.generating = false;
        void this.load();
      }
      if (e.event === "chapters_failed" && e.id === this.recordingId) {
        this.generating = false;
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
      this.chapters = await getChapters(this.recordingId);
    } catch {
      this.chapters = [];
    } finally {
      this.loading = false;
    }
  }

  /** ✨ Generate: ask the LLM for topic chapters for this recording, now. */
  private async runGenerate() {
    if (this.generating) return;
    this.generating = true;
    try {
      await suggestChapters(this.recordingId);
      // The chapters_updated event refreshes the rows; this also covers the
      // nothing-generated case (no segments / empty parse), where no event fires.
      await this.load();
    } catch (e) {
      showToast(`Chapter generation failed: ${errText(e)}`, "error");
    } finally {
      this.generating = false;
    }
  }

  /** Playback follower: highlight the chapter under the playhead. Called by the
   *  host on waveform time updates (seconds), mirroring `TimelineView`. */
  setPlaybackTime(seconds: number) {
    const idx = activeChapterIndex(this.chapters, seconds * 1000);
    if (idx !== this.activeIdx) this.activeIdx = idx;
  }

  private seek(c: Chapter) {
    this.onSeek(c.start_ms / 1000);
  }

  render() {
    if (this.loading) {
      return html`<div class="tl-loading">Loading chapters…</div>`;
    }
    const generateBtn = html`
      <button
        class="tag-manage chapters-generate"
        title="Ask the AI to divide this recording into topic chapters, anchored to the transcript timing. Re-running replaces the current set."
        ?disabled=${this.generating}
        @click=${() => void this.runGenerate()}
      >
        ${this.generating ? "✨ Generating…" : "✨ Generate chapters"}
      </button>
    `;

    if (this.chapters.length === 0) {
      return html`
        <div class="chapters">
          <div class="tags-row tags-controls">
            <span class="entities-label" style="font-size: 0.7857rem; color: var(--fg-muted);"
              title="Time-ranged topic chapters the AI derived from the transcript timing">🗂 Chapters</span>
            ${generateBtn}
          </div>
          <div class="tl-empty">
            No chapters yet — Generate to divide this recording into topic
            sections, or add the <b>Auto-chapters</b> step to a recipe to chapter
            on every recording. Needs transcript timing (re-run <b>Transcribe</b>
            to backfill it on older recordings).
          </div>
        </div>
      `;
    }

    return html`
      <div class="chapters">
        <div class="tags-row tags-controls">
          <span class="entities-label" style="font-size: 0.7857rem; color: var(--fg-muted);"
            title="Time-ranged topic chapters the AI derived from the transcript timing">🗂 Chapters</span>
          ${generateBtn}
        </div>
        <div class="tl-list" role="list">
          ${this.chapters.map(
            (c, i) => html`
              <button
                class="tl-row chapter-row${i === this.activeIdx ? " tl-active" : ""}"
                title="Jump playback to ${fmtClock(c.start_ms)}"
                @click=${() => this.seek(c)}
              >
                <span class="tl-time">${fmtClock(c.start_ms)}</span>
                <span class="chapter-body">
                  <span class="chapter-title">${c.title}</span>
                  ${c.summary
                    ? html`<span class="chapter-summary" style="display:block; font-size:0.7857rem; color: var(--fg-muted);">${c.summary}</span>`
                    : ""}
                </span>
              </button>
            `,
          )}
        </div>
      </div>
    `;
  }
}

/**
 * Imperative mount wrapper: RecordingDetail constructs one per open peek with the
 * host waveform's seek callback. `setPlaybackTime(seconds)` follows playback;
 * `dispose()` removes the element (which detaches its event subscription via
 * `disconnectedCallback`). Mirrors {@link TimelineView}'s lifecycle so the detail
 * pane drives all peeks the same way.
 */
export class ChaptersView {
  private element: ChaptersViewElement;
  constructor(
    container: HTMLElement,
    recordingId: string,
    opts: { onSeek: (seconds: number) => void },
  ) {
    this.element = document.createElement("ph-chapters-view") as ChaptersViewElement;
    this.element.recordingId = recordingId;
    this.element.onSeek = opts.onSeek;
    container.appendChild(this.element);
  }

  /** Highlight the chapter under the playhead (host waveform time, seconds). */
  setPlaybackTime(seconds: number) {
    this.element.setPlaybackTime(seconds);
  }

  dispose() {
    this.element.remove();
  }
}
