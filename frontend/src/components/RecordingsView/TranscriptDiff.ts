import { escapeHtml } from "../../utils/format";
import { diffTextDetailed, type DiffOp, type DiffOpType, type DiffMode, type DiffOutcome } from "../../utils/diff";

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

/** One of the three comparable transcript layers (see the file-top comment). */
export type LayerKey = "original" | "clean" | "current";

/** The already-resolved text of each layer, as the host fetched it. */
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

/** The diff view's controller. Plain class: RecordingDetail mounts one with
 *  the pre-fetched layers; it owns the layer pickers, the word↔line mode
 *  toggle, and re-rendering — fully self-contained and read-only from there
 *  (no IPC, no events). Unmount by clearing the container. */
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
    // Compute the (capped) diff once and feed BOTH the body and the stats from
    // it — the two used to each run `diffTextDetailed` independently at the same
    // granularity, doubling the LCS work on every render/refresh.
    const outcome = this.computeDiff();
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
          <span class="tdiff-stat" id="tdiff-stats">${this.statsHtml(outcome)}</span>
          <span class="tdiff-spacer"></span>
          <span class="tdiff-key tdiff-del">removed</span>
          <span class="tdiff-key tdiff-ins">added</span>
        </div>
        <div class="tdiff-body" id="tdiff-body">${this.bodyHtml(outcome)}</div>
      </div>
    `;
    this.wire();
  }

  /**
   * The diff for the currently-selected sides + mode, or `null` when there's
   * nothing to compare (a missing layer, or the same layer on both sides). The
   * single source of truth for both the body and the stats so the LCS runs once
   * per render.
   */
  private computeDiff(): DiffOutcome | null {
    if (this.left === this.right || !this.hasContent(this.left) || !this.hasContent(this.right)) {
      return null;
    }
    return diffTextDetailed(this.valueOf(this.left) ?? "", this.valueOf(this.right) ?? "", this.mode);
  }

  private selectHtml(side: "left" | "right", selected: LayerKey): string {
    const opts = LAYER_ORDER.map((k) => {
      const missing = !this.hasContent(k);
      const label = LAYER_LABELS[k] + (missing ? " — n/a" : "");
      return `<option value="${k}"${k === selected ? " selected" : ""}>${escapeHtml(label)}</option>`;
    }).join("");
    return `<select class="tdiff-select" data-side="${side}" aria-label="${side} version">${opts}</select>`;
  }

  /** The diff body, or a clear empty/unavailable state. `outcome` is the
   *  shared precomputed diff (`null` when there's nothing to compare). */
  private bodyHtml(outcome: DiffOutcome | null): string {
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

    if (this.left === this.right || !outcome) {
      return `<div class="tdiff-empty">Pick two different versions to compare.</div>`;
    }

    // Size-guarded: hour-long transcripts can exceed the word-level LCS cap,
    // in which case the diff arrives at a coarser granularity — say so rather
    // than letting the view silently look less precise than the mode toggle.
    const { ops, fallback } = outcome;
    const note =
      fallback === "line"
        ? `<div class="tdiff-empty">These versions are too long for a word-level diff — showing line-level changes instead.</div>`
        : fallback === "block"
          ? `<div class="tdiff-empty">These versions are too long for a detailed diff — showing the changed region as one block.</div>`
          : "";
    if (ops.every((o) => o.type === "equal")) {
      return `${note}<div class="tdiff-same">These two versions are identical.</div>
        <div class="tdiff-text tdiff-text--numbered">${this.renderUnified(ops)}</div>`;
    }
    return `${note}<div class="tdiff-text tdiff-text--numbered">${this.renderUnified(ops)}</div>`;
  }

  /** Render a unified, line-numbered diff: an old + new line-number gutter, a
   *  per-line marker (+ added · − removed · ~ changed-in-place · blank context),
   *  and the row's inline word/line highlights inside the content column. */
  private renderUnified(ops: DiffOp[]): string {
    type Seg = { type: DiffOpType; text: string };
    type Line = { segs: Seg[]; hasOld: boolean; hasNew: boolean };

    // Split the op stream into logical lines, tracking whether each line exists
    // in the old side (equal|delete) and/or the new side (equal|insert) so the
    // two gutters advance correctly — even for blank lines with no segments.
    const lines: Line[] = [];
    let segs: Seg[] = [];
    let hasOld = false;
    let hasNew = false;
    const finalize = () => {
      lines.push({ segs, hasOld, hasNew });
      segs = [];
      hasOld = false;
      hasNew = false;
    };
    for (const op of ops) {
      const isOld = op.type === "equal" || op.type === "delete";
      const isNew = op.type === "equal" || op.type === "insert";
      const parts = op.value.split("\n");
      for (let p = 0; p < parts.length; p++) {
        const text = parts[p];
        const endsLine = p < parts.length - 1;
        if (endsLine || text !== "") {
          if (isOld) hasOld = true;
          if (isNew) hasNew = true;
          if (text) segs.push({ type: op.type, text });
        }
        if (endsLine) finalize();
      }
    }
    if (segs.length || hasOld || hasNew) finalize();

    let oldNo = 0;
    let newNo = 0;
    const cls = (t: DiffOpType) => (t === "insert" ? "ins" : t === "delete" ? "del" : "eq");
    return lines
      .map((line) => {
        const oldNum = line.hasOld ? (++oldNo).toString() : "";
        const newNum = line.hasNew ? (++newNo).toString() : "";
        const ins = line.segs.some((s) => s.type === "insert");
        const del = line.segs.some((s) => s.type === "delete");
        let kind = "ctx";
        let mark = " ";
        if (ins || del) {
          if (line.hasOld && line.hasNew) { kind = "mod"; mark = "~"; }
          else if (line.hasNew) { kind = "add"; mark = "+"; }
          else { kind = "del"; mark = "−"; }
        }
        const content =
          line.segs.map((s) => `<span class="tdiff-${cls(s.type)}">${escapeHtml(s.text)}</span>`).join("") ||
          "&nbsp;";
        return (
          `<div class="tdiff-row tdiff-row--${kind}">` +
          `<span class="tdiff-ln">${oldNum}</span>` +
          `<span class="tdiff-ln">${newNum}</span>` +
          `<span class="tdiff-mark">${mark}</span>` +
          `<span class="tdiff-content">${content}</span>` +
          `</div>`
        );
      })
      .join("");
  }

  /** A short "+N added · −M removed" (by word count) summary of the diff.
   *  Reuses the shared precomputed `outcome` instead of re-diffing. */
  private statsHtml(outcome: DiffOutcome | null): string {
    if (!outcome) return "";
    const ops = outcome.ops;
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

  /** Re-render the diff body + the stats (after a picker/mode/swap change).
   *  Computes the diff once and feeds both, like `render`. */
  private refresh() {
    const outcome = this.computeDiff();
    const body = this.container.querySelector<HTMLElement>("#tdiff-body");
    if (body) body.innerHTML = this.bodyHtml(outcome);
    const stats = this.container.querySelector<HTMLElement>("#tdiff-stats");
    if (stats) stats.innerHTML = this.statsHtml(outcome);
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
