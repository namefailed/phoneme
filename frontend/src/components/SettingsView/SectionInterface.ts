import { renderField, bindFieldEvents } from "./form";

/** All reorderable/toggleable list columns. */
const COLUMN_CATALOG: { value: string; label: string }[] = [
  { value: "day", label: "Day" },
  { value: "time", label: "Time" },
  { value: "duration", label: "Duration" },
  { value: "status", label: "Status" },
  { value: "tags", label: "Tags" },
  { value: "model", label: "Transcription Model" },
  { value: "cleanup_model", label: "Post-Processing Model" },
  { value: "summary_model", label: "Summary Model" },
  { value: "diarized", label: "Diarized" },
  { value: "user_edited", label: "Edited" },
  { value: "transcript", label: "Transcript Snippet" },
];

export class SectionInterface {
  private container: HTMLElement;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private config: any;
  /** Working order of ALL toggleable columns (visible ones first, in their
   *  saved order, then the hidden ones). Drives both the list and the saved
   *  `visible_columns` order. */
  private order: string[] = [];
  private visible = new Set<string>();

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    this.container = container;
    this.config = config;
    if (!config.interface) {
      config.interface = {
        theme: "catppuccin-mocha",
        format_24h: false,
        strip_titlebar: false,
        visible_columns: ["day", "time", "duration", "status", "transcript"],
      };
    }

    const known = new Set(COLUMN_CATALOG.map((c) => c.value));
    const saved: string[] = (config.interface.visible_columns || []).filter((c: string) => known.has(c));
    // Visible columns first (saved order), then any remaining hidden columns.
    this.order = [
      ...saved,
      ...COLUMN_CATALOG.map((c) => c.value).filter((c) => !saved.includes(c)),
    ];
    this.visible = new Set(saved);

    this.renderShell();
    this.renderColumns();
  }

  private label(value: string): string {
    return COLUMN_CATALOG.find((c) => c.value === value)?.label ?? value;
  }

  /** Persist the current order + visibility into config and notify SettingsView. */
  private syncConfig() {
    this.config.interface.visible_columns = this.order.filter((c) => this.visible.has(c));
    // Saved widths are positional, so dropping/reordering columns would
    // misalign them. Clear them so the list recomputes per-column widths in the
    // new order (the user can re-drag widths afterward).
    if (this.config.interface.column_widths) delete this.config.interface.column_widths;
    // Bubbling change so SettingsView enables the Save button.
    this.container.dispatchEvent(new Event("change", { bubbles: true }));
  }

  private move(index: number, dir: -1 | 1) {
    const j = index + dir;
    if (j < 0 || j >= this.order.length) return;
    [this.order[index], this.order[j]] = [this.order[j], this.order[index]];
    this.renderColumns();
    this.syncConfig();
  }

  private renderColumns() {
    const host = this.container.querySelector<HTMLElement>("#col-list");
    if (!host) return;
    host.innerHTML = this.order
      .map((value, i) => `
          <div class="col-row" data-col="${value}">
            <label class="col-label">
              <input type="checkbox" class="col-toggle" value="${value}" ${this.visible.has(value) ? "checked" : ""} />
              <span>${this.label(value)}</span>
            </label>
            <span class="col-move">
              <button class="col-up" title="Move up" data-i="${i}" ${i === 0 ? "disabled" : ""}>▲</button>
              <button class="col-down" title="Move down" data-i="${i}" ${i === this.order.length - 1 ? "disabled" : ""}>▼</button>
            </span>
          </div>`)
      .join("");

    host.querySelectorAll<HTMLButtonElement>(".col-up").forEach((b) =>
      b.addEventListener("click", () => this.move(Number(b.dataset.i), -1)),
    );
    host.querySelectorAll<HTMLButtonElement>(".col-down").forEach((b) =>
      b.addEventListener("click", () => this.move(Number(b.dataset.i), 1)),
    );
    host.querySelectorAll<HTMLInputElement>(".col-toggle").forEach((cb) =>
      cb.addEventListener("change", () => {
        if (cb.checked) this.visible.add(cb.value);
        else this.visible.delete(cb.value);
        this.syncConfig();
      }),
    );
  }

  private renderShell() {
    const config = this.config;
    this.container.innerHTML = `
      <div class="settings-section">
        <style>
          #col-list { max-width: 320px; }
          #col-list .col-row {
            display: flex; align-items: center; justify-content: space-between; gap: 10px;
            padding: 3px 6px; border-radius: 6px; transition: background 0.12s ease;
          }
          #col-list .col-row:hover { background: color-mix(in srgb, var(--accent) 7%, transparent); }
          #col-list .col-move { display: inline-flex; flex-direction: column; gap: 1px; }
          #col-list .col-move button {
            background: transparent; border: none; color: var(--fg-faded);
            width: 22px; height: 14px; line-height: 1; font-size: 9px; padding: 0;
            border-radius: 4px; cursor: pointer; transition: background 0.12s ease, color 0.12s ease;
          }
          #col-list .col-move button:hover:not(:disabled) { background: color-mix(in srgb, var(--accent) 20%, transparent); color: var(--accent); }
          #col-list .col-move button:disabled { opacity: 0.25; cursor: default; }
          #col-list .col-label { display: flex; align-items: center; gap: 8px; font-weight: normal; cursor: pointer; }
        </style>
        <h3>Interface</h3>

        <div class="settings-field">
          <label>Theme</label>
          <div>
            ${renderField(
              {
                key: "interface.theme",
                label: "Theme",
                kind: "select",
                options: [
                  { value: "catppuccin-mocha",    label: "Catppuccin Mocha" },
                  { value: "catppuccin-macchiato", label: "Catppuccin Macchiato" },
                  { value: "dracula",              label: "Dracula" },
                  { value: "everforest",           label: "Everforest" },
                  { value: "gruvbox",              label: "Gruvbox" },
                  { value: "nord",                 label: "Nord" },
                  { value: "one-dark",             label: "One Dark" },
                  { value: "rose-pine",            label: "Rosé Pine" },
                  { value: "tokyo-night",          label: "Tokyo Night" },
                  { value: "catppuccin-latte",     label: "Catppuccin Latte (Light)" },
                  { value: "solarized-light",      label: "Solarized Light" },
                ],
              },
              config.interface.theme,
            )}
          </div>
        </div>

        <div class="settings-field">
          <label>24-hour time format</label>
          <div>${renderField(
            { key: "interface.format_24h", label: "", kind: "checkbox" },
            config.interface.format_24h,
          )}</div>
        </div>

        <div class="settings-field">
          <label>Strip system titlebar</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "interface.strip_titlebar", label: "", kind: "checkbox" },
              config.interface.strip_titlebar,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Removes the default OS window decorations. The top header will become draggable. Requires app restart to fully apply.
            </span>
          </div>
        </div>

        <div class="settings-field" style="align-items: flex-start;">
          <label style="margin-top: 8px;">Visible Columns</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 6px; width: 100%;">
            <div id="col-list" style="display: flex; flex-direction: column; gap: 8px;"></div>
            <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
              Check a column to show it; use ▲▼ to reorder. Columns appear left-to-right in this order.
            </span>
          </div>
        </div>

      </div>
    `;
    bindFieldEvents(this.container, config);
  }
}
