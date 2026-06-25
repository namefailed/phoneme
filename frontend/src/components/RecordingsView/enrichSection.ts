import { html, type TemplateResult } from "lit";

/**
 * Shared chrome for the detail pane's AI-enrichment sections (Entities, Tasks):
 * a collapsible section header that matches the sidebar's collapse pattern
 * (chevron rotates, accent on hover) plus localStorage-backed collapse memory.
 *
 * Both sections render the same header via {@link enrichHead} so they stay
 * pixel-identical, and persist their open/closed state per section name so a
 * collapse survives reloads and recording switches.
 */

/** Right-pointing disclosure chevron; rotates to "down" via the `.open` class,
 *  exactly like `.sidebar-chevron`. */
export const ENRICH_CHEVRON = html`<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="9 6 15 12 9 18"></polyline></svg>`;

/**
 * A tinted square identity badge for a section (Tasks = green check, Entities =
 * blue search). The glyph is an inline SVG so it inherits the section's `--tint`
 * cleanly across every theme — unlike the old emoji baked into the label string.
 * `tint` is a token name (e.g. `--ok`); `glyph` is the SVG inner markup.
 */
export function enrichBadge(tint: string, glyph: TemplateResult): TemplateResult {
  return html`<span class="enrich-badge" style="--tint: var(${tint});">${glyph}</span>`;
}

const key = (name: string) => `phoneme.enrich.${name}.collapsed`;

/** Whether the named section was last left collapsed (default expanded). */
export function loadCollapsed(name: string): boolean {
  try {
    return localStorage.getItem(key(name)) === "1";
  } catch {
    return false;
  }
}

/** Remember the named section's collapsed state. */
export function saveCollapsed(name: string, collapsed: boolean): void {
  try {
    localStorage.setItem(key(name), collapsed ? "1" : "0");
  } catch {
    /* localStorage may be unavailable; collapse just won't persist */
  }
}

/**
 * The section header row: a click-to-collapse button on the left (chevron +
 * label + optional count) and an action slot on the right (the Extract button).
 * The action is a sibling of the toggle button, so clicking Extract never also
 * toggles the collapse.
 */
export function enrichHead(opts: {
  label: string;
  collapsed: boolean;
  onToggle: () => void;
  count?: TemplateResult | string;
  action?: TemplateResult;
  badge?: TemplateResult;
}): TemplateResult {
  return html`
    <div class="enrich-head">
      <button
        class="enrich-toggle"
        aria-expanded=${!opts.collapsed}
        title=${opts.collapsed ? "Expand" : "Collapse"}
        @click=${opts.onToggle}
      >
        <span class="enrich-chevron ${opts.collapsed ? "" : "open"}">${ENRICH_CHEVRON}</span>
        ${opts.badge ?? ""}
        <span class="enrich-label">${opts.label}</span>
        ${opts.count != null ? html`<span class="enrich-count">${opts.count}</span>` : ""}
      </button>
      ${opts.action ? html`<div class="enrich-actions">${opts.action}</div>` : ""}
    </div>
  `;
}
