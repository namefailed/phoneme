import { errText } from "../../utils/error";
import { LitElement, html, PropertyValues } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { getRecording, suggestEntities, type Entity } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { showToast } from "../../utils/toast";

/** The entity classes, in display order, with their group label + icon. An
 *  unknown kind from the model is stored as `topic` by the daemon, so this list
 *  is exhaustive for what can land. */
const KIND_META: Array<{ kind: string; label: string; icon: string }> = [
  { kind: "person", label: "People", icon: "👤" },
  { kind: "org", label: "Organizations", icon: "🏢" },
  { kind: "topic", label: "Topics", icon: "💡" },
  { kind: "term", label: "Terms", icon: "🔤" },
];

/**
 * The detail pane's entities surface: the recording's structured, typed entities
 * (person / org / topic / term) rendered as read-only chips grouped by kind, with
 * a 🔎 Extract button that runs the LLM entity-extraction step on demand.
 *
 * Richer than the flat auto-tag chips ({@link TagChips}): entities carry a type,
 * so they group. Read-only by design — the daemon owns the entity set (the
 * extraction step writes it, re-running replaces it); the UI just surfaces it.
 *
 * Loads its own data per `recordingId` (off `getRecording`) and live-refreshes on
 * the `entities_updated` daemon event, so a pipeline run or an on-demand extract
 * elsewhere updates the chips. Errors toast.
 */
@customElement("ph-entity-chips")
export class EntityChipsElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM, to inherit the global tag-chip styles.
  }

  @property({ type: String }) recordingId = "";

  @state() private entities: Entity[] = [];
  /** True while an on-demand 🔎 Extract run is in flight. */
  @state() private extracting = false;
  private unsubEvents: (() => void) | null = null;

  connectedCallback() {
    super.connectedCallback();
    if (this.recordingId) void this.load();
    void subscribe((e: DaemonEvent) => {
      if (e.event === "entities_updated" && e.id === this.recordingId) {
        this.extracting = false;
        void this.load();
      }
      if (e.event === "entities_failed" && e.id === this.recordingId) {
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
      this.entities = rec.entities ?? [];
    } catch {
      this.entities = [];
    }
  }

  /** 🔎 Extract: ask the LLM for structured entities for this recording, now. */
  private async runExtract() {
    if (this.extracting) return;
    this.extracting = true;
    try {
      await suggestEntities(this.recordingId);
      // The entities_updated event refreshes the chips; this covers the
      // nothing-extracted case (no event fires then).
      await this.load();
    } catch (e) {
      showToast(`Entity extraction failed: ${errText(e)}`, "error");
    } finally {
      this.extracting = false;
    }
  }

  render() {
    // Group the flat list by kind, in KIND_META order; an unexpected kind falls
    // into its own trailing group rather than being dropped.
    const byKind = new Map<string, string[]>();
    for (const e of this.entities) {
      const list = byKind.get(e.kind) ?? [];
      list.push(e.value);
      byKind.set(e.kind, list);
    }
    const orderedKinds = [
      ...KIND_META.map((m) => m.kind).filter((k) => byKind.has(k)),
      ...[...byKind.keys()].filter((k) => !KIND_META.some((m) => m.kind === k)),
    ];

    return html`
      <div class="entities">
        <div class="tags-row tags-controls">
          <span class="entities-label" title="Structured entities the AI extracted from this transcript (read-only)"
            style="font-size: 0.7857rem; color: var(--fg-muted);">🔎 Entities</span>
          <button class="tag-manage entity-extract"
            title="Ask the AI to extract structured entities (people, orgs, topics, terms) from this recording. Re-running replaces the current set."
            ?disabled=${this.extracting} @click=${() => void this.runExtract()}>${this.extracting ? "🔎 Extracting…" : "🔎 Extract"}</button>
        </div>
        ${orderedKinds.length
          ? html`<div class="entities-groups">
              ${orderedKinds.map((kind) => {
                const meta = KIND_META.find((m) => m.kind === kind);
                const values = byKind.get(kind) ?? [];
                return html`
                  <div class="entity-group" style="display:flex; flex-wrap:wrap; align-items:center; gap:6px; margin-top:6px;">
                    <span class="entity-group-label" title=${meta?.label ?? kind}
                      style="font-size: 0.7857rem; color: var(--fg-muted); margin-right:2px;">${meta?.icon ?? "•"} ${meta?.label ?? kind}:</span>
                    ${values.map(
                      (v) => html`<span class="tag-chip tag-chip--entity" data-entity-kind=${kind}>${v}</span>`,
                    )}
                  </div>
                `;
              })}
            </div>`
          : html`<div class="entities-empty" style="font-size: 0.7857rem; color: var(--fg-muted); margin-top:4px;">No entities yet — Extract to pull people, orgs, topics, and terms from the transcript.</div>`}
      </div>
    `;
  }
}

/** Imperative mount wrapper: RecordingDetail creates one per render; the element
 *  manages its own data from there. Mirrors {@link TagChips}. */
export class EntityChips {
  private element: EntityChipsElement;
  constructor(container: HTMLElement, recordingId: string) {
    this.element = document.createElement("ph-entity-chips") as EntityChipsElement;
    this.element.recordingId = recordingId;
    container.appendChild(this.element);
  }
}
