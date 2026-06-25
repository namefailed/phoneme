import { LitElement, html, type PropertyValues, type TemplateResult } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { getRecording, getEntities } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { ENRICH_CHEVRON, loadCollapsed, saveCollapsed } from "./enrichSection";
// Side-effect imports: register <ph-task-chips> / <ph-entity-chips>.
import "./TaskChips";
import "./EntityChips";

/** Small check glyph for the Tasks summary pill (tinted via inline color). */
const CHECK_GLYPH = html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="20 6 9 17 4 12"></polyline></svg>`;

/** Entity kinds for the collapsed summary, in display order, with their kind
 *  emoji (matching the per-kind group rows) and the tint their count uses. */
const KIND_SUMMARY: Array<{ kind: string; emoji: string; tint: string; label: string }> = [
  { kind: "person", emoji: "👤", tint: "--info", label: "People" },
  { kind: "org", emoji: "🏢", tint: "--ok", label: "Organizations" },
  { kind: "topic", emoji: "💡", tint: "--peach", label: "Topics" },
  { kind: "term", emoji: "🔤", tint: "--accent", label: "Terms" },
];

/**
 * The detail pane's "Insights" card — one bordered card (cloning the notes /
 * transcript card chrome) that hosts the Tasks and Entities sub-sections so the
 * three cards read as one rhythmic stack instead of loose dividers.
 *
 * Three-tier minimize: this card's own collapse folds everything to a single
 * header bar (kept under `phoneme.enrich.card.collapsed`), and each child keeps
 * its own per-section collapse. The summary cluster (open/total tasks + per-kind
 * entity counts) shows in both the collapsed bar and the expanded header, so the
 * card fetches its own lightweight counts (getRecording + getEntities — cheap,
 * local IPC; when expanded the mounted children re-fetch the same data, a small
 * deliberate redundancy). It live-refreshes on the same daemon events the
 * children listen to.
 */
@customElement("ph-insights-card")
export class InsightsCardElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM, to inherit the global card + chip styles.
  }

  @property({ type: String }) recordingId = "";

  @state() private collapsed = loadCollapsed("card");
  @state() private taskOpen = 0;
  @state() private taskTotal = 0;
  @state() private entityByKind: Record<string, number> = {};
  private unsubEvents: (() => void) | null = null;

  private toggle = () => {
    this.collapsed = !this.collapsed;
    saveCollapsed("card", this.collapsed);
  };

  connectedCallback() {
    super.connectedCallback();
    // Initial counts load is driven by the first updated() (recordingId change),
    // so it isn't duplicated here.
    void subscribe((e: DaemonEvent) => {
      // tasks/entities updates carry this recording's id; a library-wide merge is
      // global (it can rename/remove this recording's entities too).
      if (e.event === "tasks_updated" && e.id === this.recordingId) void this.loadCounts();
      else if (e.event === "entities_updated" && e.id === this.recordingId) void this.loadCounts();
      else if (e.event === "entities_merged") void this.loadCounts();
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
    if (changed.has("recordingId") && this.recordingId) void this.loadCounts();
  }

  /** Fetch just the counts the summary needs (tasks off the row, entities off the
   *  lightweight endpoint). Failure leaves the last-known counts. */
  private async loadCounts() {
    try {
      const [rec, ents] = await Promise.all([
        getRecording(this.recordingId),
        getEntities(this.recordingId),
      ]);
      const tasks = rec.tasks ?? [];
      this.taskTotal = tasks.length;
      this.taskOpen = tasks.filter((t) => !t.done).length;
      const by: Record<string, number> = {};
      for (const e of ents) by[e.kind] = (by[e.kind] ?? 0) + 1;
      this.entityByKind = by;
    } catch {
      /* keep last-known counts */
    }
  }

  private renderSummary(): TemplateResult | string {
    const pills: TemplateResult[] = [];
    if (this.taskTotal > 0) {
      pills.push(html`<span class="insights-pill" title="${this.taskOpen} open of ${this.taskTotal} tasks">
        <span class="ip-glyph" style="color: var(--ok);">${CHECK_GLYPH}</span>${this.taskOpen}/${this.taskTotal}
      </span>`);
    }
    const kinds = KIND_SUMMARY.filter((k) => (this.entityByKind[k.kind] ?? 0) > 0);
    if (kinds.length) {
      pills.push(html`<span class="insights-pill">
        ${kinds.map(
          (k) => html`<span class="ip-kind" title="${k.label}: ${this.entityByKind[k.kind]}" aria-label="${k.label}: ${this.entityByKind[k.kind]}"><span class="ip-emoji" aria-hidden="true">${k.emoji}</span><span style="color: color-mix(in srgb, var(${k.tint}) 72%, var(--fg-default));">${this.entityByKind[k.kind]}</span></span>`
        )}
      </span>`);
    }
    return pills.length ? html`${pills}` : "";
  }

  render() {
    return html`
      <div class="insights-card ${this.collapsed ? "is-collapsed" : ""}">
        <button
          class="insights-head"
          aria-expanded=${!this.collapsed}
          title=${this.collapsed ? "Expand insights" : "Collapse insights"}
          @click=${this.toggle}
        >
          <span class="enrich-chevron ${this.collapsed ? "" : "open"}">${ENRICH_CHEVRON}</span>
          <span class="insights-title detail-section-title">Insights</span>
          <span class="insights-summary">${this.renderSummary()}</span>
        </button>
        ${this.collapsed
          ? ""
          : html`<div class="insights-body">
              <ph-task-chips .recordingId=${this.recordingId}></ph-task-chips>
              <div class="enrich-sep"></div>
              <ph-entity-chips .recordingId=${this.recordingId}></ph-entity-chips>
            </div>`}
      </div>
    `;
  }
}

/** Imperative mount wrapper: RecordingDetail creates one per render; the element
 *  manages its own data + children from there. Mirrors {@link TaskChips}. */
export class InsightsCard {
  private element: InsightsCardElement;
  constructor(container: HTMLElement, recordingId: string) {
    this.element = document.createElement("ph-insights-card") as InsightsCardElement;
    this.element.recordingId = recordingId;
    container.appendChild(this.element);
  }
}
