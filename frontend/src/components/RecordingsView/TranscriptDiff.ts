import { escapeHtml } from "../../utils/format";
import { diffText, type DiffOp, type DiffMode } from "../../utils/diff";

/**
 * Read-only side-by-side-ish DIFF of a recording's transcript layers
 * (roadmap v1.10 — "Compare transcript versions").
 *
 * Three layers can exist for a recording:
 *   • original — the raw machine (Whisper) transcript, before AI cleanup
 *   • clean    — the LLM-cleaned transcript, before the user's hand edits
 *   • current  — the live transcript (possibly hand-edited)
 *
 * The user picks any two of the three and gets an inline word- (or line-) level
 * diff with clear insertion/deletion highlighting. Everything is read-only — the
 * component never writes a transcript back. Layers that don't exist for this
 * recording (e.g. cleanup never ran) are still offered in the pickers but show a
 * clear "not available" state instead of rendering a broken/empty diff.
 *
 * Self-contained: pass the three already-resolved layer values in; the host
 * (RecordingDetail) fetches `original`/`clean` once via IPC before mounting.
 */

export type LayerKey = "original" | "clean" | "current";

export interface TranscriptLayers {
  /** Raw machine transcript, or null/empty if none was preserved. */
  original: string | null;
  /** LLM-cleaned (pre-edit) transcript, or null/empty if cleanup never ran. */
  clean: string | null;
  /** The current (possibly edited) transcript. */
  current: string | null;
}

const LAYER_LABELS: Record<LayerKey, string> = {
  original: "Original (raw machine)",
  clean: "Cleaned (pre-edit)",
  current: "Current",
};

const LAYER_ORDER: LayerKey[] = ["original", "clean", "current"];

export class TranscriptDiff {
  private container: HTMLElement;
  private layers: TranscriptLayers;
  private left: LayerKey;
  private right: LayerKey;
  private mode: DiffMode = "word";

  constructor(container: HTMLElement, layers: TranscriptLayers) {
    this.container = container;
    this.layers = layers;
    // Default: original ↔ current (the most useful "what changed overall?" view).
    // If a side's default layer is missing, fall back to the first available one
    // so the diff isn't pointless on first open.
    this.left = this.firstAvailable(["original", "clean", "current"]);
    this.right = this.firstAvailable(["current", "clean", "original"]);
    this.render();
  }

  /** Pick the first layer in `prefs` that has content, else the first pref. */
  private firstAvailable(prefs: LayerKey[]): LayerKey {
    return prefs.find((k) => this.hasContent(k)) ?? prefs[0];
  }

  private valueOf(key: LayerKey): string | null {
    return this.layers[key];
  }

  private hasContent(key: LayerKey): boolean {
    const v = this.valueOf(key);
    return v != null && v.trim().length > 0;
  }

  private render() {
    this.container.innerHTML = `
      <div class="tdiff">
        <div class="tdiff-bar">
          <div class="tdiff-pickers">
            ${this.selectHtml("left", this.left)}
            <button class="tdiff-swap" title="Swap sides" aria-label="Swap the two versions">⇄</button>
            ${this.selectHtml("right", this.right)}
          </div>
          <div class="tdiff-modes" role="group" aria-label="Diff granularity">
            <button class="tdiff-mode ${this.mode === "word" ? "active" : ""}" data-mode="word">Words</button>
            <button class="tdiff-mode ${this.mode === "line" ? "active" : ""}" data-mode="line">Lines</button>
          </div>
        </div>
        <div class="tdiff-legend">
          <span class="tdiff-stat" id="tdiff-stats">${this.statsHtml()}</span>
          <span class="tdiff-spacer"></span>
          <span class="tdiff-key tdiff-del">removed</span>
          <span class="tdiff-key tdiff-ins">added</span>
        </div>
        <div class="tdiff-body" id="tdiff-body">${this.bodyHtml()}</div>
      </div>
    `;
    this.wire();
  }

  private selectHtml(side: "left" | "right", selected: LayerKey): string {
    const opts = LAYER_ORDER.map((k) => {
      const missing = !this.hasContent(k);
      const label = LAYER_LABELS[k] + (missing ? " — n/a" : "");
      return `<option value="${k}"${k === selected ? " selected" : ""}>${escapeHtml(label)}</option>`;
    }).join("");
    return `<select class="tdiff-select" data-side="${side}" aria-label="${side} version">${opts}</select>`;
  }

  /** The diff body, or a clear empty/unavailable state. */
  private bodyHtml(): string {
    const leftVal = this.valueOf(this.left);
    const rightVal = this.valueOf(this.right);

    // A layer that was never saved (null) vs one that's merely empty are both
    // "nothing to compare", but the message is friendlier when we name which
    // side is missing.
    const missing: string[] = [];
    if (!this.hasContent(this.left)) missing.push(LAYER_LABELS[this.left]);
    if (!this.hasContent(this.right) && this.right !== this.left) missing.push(LAYER_LABELS[this.right]);
    if (missing.length > 0) {
      const which = missing.join(" and ");
      return `<div class="tdiff-empty">No ${escapeHtml(which.toLowerCase())} version is available for this recording, so there's nothing to compare.</div>`;
    }

    if (this.left === this.right) {
      return `<div class="tdiff-empty">Pick two different versions to compare.</div>`;
    }

    const ops = diffText(leftVal ?? "", rightVal ?? "", this.mode);
    if (ops.every((o) => o.type === "equal")) {
      return `<div class="tdiff-same">These two versions are identical.</div>
        <div class="tdiff-text">${this.renderOps(ops)}</div>`;
    }
    return `<div class="tdiff-text">${this.renderOps(ops)}</div>`;
  }

  /** Turn diff ops into highlighted, HTML-escaped spans. */
  private renderOps(ops: DiffOp[]): string {
    return ops
      .map((op) => {
        const safe = escapeHtml(op.value);
        if (op.type === "insert") return `<span class="tdiff-ins">${safe}</span>`;
        if (op.type === "delete") return `<span class="tdiff-del">${safe}</span>`;
        return `<span class="tdiff-eq">${safe}</span>`;
      })
      .join("");
  }

  /** A short "+N added · −M removed" (by word count) summary of the diff. */
  private statsHtml(): string {
    if (this.left === this.right || !this.hasContent(this.left) || !this.hasContent(this.right)) {
      return "";
    }
    const ops = diffText(this.valueOf(this.left) ?? "", this.valueOf(this.right) ?? "", this.mode);
    const words = (s: string) => (s.trim() ? s.trim().split(/\s+/).length : 0);
    let added = 0;
    let removed = 0;
    for (const op of ops) {
      if (op.type === "insert") added += words(op.value);
      else if (op.type === "delete") removed += words(op.value);
    }
    if (added === 0 && removed === 0) return `<span class="tdiff-stat-same">No differences</span>`;
    return `<span class="tdiff-stat-add">+${added} added</span> · <span class="tdiff-stat-del">−${removed} removed</span>`;
  }

  /** Re-render the diff body + the stats (after a picker/mode/swap change). */
  private refresh() {
    const body = this.container.querySelector<HTMLElement>("#tdiff-body");
    if (body) body.innerHTML = this.bodyHtml();
    const stats = this.container.querySelector<HTMLElement>("#tdiff-stats");
    if (stats) stats.innerHTML = this.statsHtml();
  }

  private wire() {
    this.container.querySelectorAll<HTMLSelectElement>(".tdiff-select").forEach((sel) => {
      sel.addEventListener("change", () => {
        const side = sel.dataset.side as "left" | "right";
        const key = sel.value as LayerKey;
        if (side === "left") this.left = key;
        else this.right = key;
        this.refresh();
      });
    });
    this.container.querySelector<HTMLButtonElement>(".tdiff-swap")?.addEventListener("click", () => {
      [this.left, this.right] = [this.right, this.left];
      this.render();
    });
    this.container.querySelectorAll<HTMLButtonElement>(".tdiff-mode").forEach((btn) => {
      btn.addEventListener("click", () => {
        const next = btn.dataset.mode as DiffMode;
        if (next === this.mode) return;
        this.mode = next;
        // Toggle the active class without a full re-render so the pickers keep
        // their state, then refresh the diff body + stats.
        this.container.querySelectorAll(".tdiff-mode").forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
        this.refresh();
      });
    });
  }
}
