import { errText } from "../../utils/error";
import { LitElement, html, PropertyValues } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { getRecording, suggestEntities, type Entity } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { showToast } from "../../utils/toast";
import { enrichHead, loadCollapsed, saveCollapsed } from "./enrichSection";

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
  /** Section collapsed (remembered across reloads + recording switches). */
  @state() private collapsed = loadCollapsed("entities");
  private unsubEvents: (() => void) | null = null;

  private toggleCollapsed = () => {
    this.collapsed = !this.collapsed;
    saveCollapsed("entities", this.collapsed);
  };

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
    // Flow the chips ordered by kind (KIND_META order) so same-kind entities sit
    // together and share a colour; an unexpected kind sorts to the end. Each chip
    // carries its kind for the icon + per-kind colour.
    const kindRank = (k: string) => {
      const i = KIND_META.findIndex((m) => m.kind === k);
      return i === -1 ? KIND_META.length : i;
    };
    const ordered = [...this.entities].sort((a, b) => kindRank(a.kind) - kindRank(b.kind));
    const total = this.entities.length;

    return html`
      <div class="detail-enrich entities ${this.collapsed ? "is-collapsed" : ""}">
        ${enrichHead({
          label: "🔎 Entities",
          collapsed: this.collapsed,
          onToggle: this.toggleCollapsed,
          count: total ? String(total) : undefined,
          action: html`<button class="tag-manage entity-extract"
            title="Ask the AI to extract structured entities (people, orgs, topics, terms) from this recording. Re-running replaces the current set."
            ?disabled=${this.extracting} @click=${() => void this.runExtract()}>${this.extracting ? "🔎 Extracting…" : "🔎 Extract"}</button>`,
        })}
        ${this.collapsed
          ? ""
          : total
            ? html`<div class="enrich-body entity-chips-flow">
                ${ordered.map((e) => {
                  const meta = KIND_META.find((m) => m.kind === e.kind);
                  return html`<span class="tag-chip tag-chip--entity" data-entity-kind=${e.kind} title=${meta?.label ?? e.kind}><span class="ent-ico">${meta?.icon ?? "•"}</span>${e.value}</span>`;
                })}
              </div>`
            : html`<div class="enrich-body enrich-empty">No entities yet — Extract to pull people, orgs, topics, and terms from the transcript.</div>`}
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
