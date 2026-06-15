import { renderField, bindFieldEvents } from "./form";
import { invoke } from "@tauri-apps/api/core";

/** Default visible columns, used by the reset action. */
const DEFAULT_VISIBLE_COLUMNS = ["day", "time", "duration", "status", "source", "transcript"];

/** Curated UI font choices. Empty value = the bundled default (Inter). The rest
 *  are families that ship with Windows (the primary target) plus common
 *  cross-platform picks, so a selection renders without installing anything. */
const UI_FONT_CATALOG: { value: string; label: string }[] = [
  { value: "", label: "System default (Inter)" },
  { value: "Segoe UI", label: "Segoe UI" },
  { value: "Calibri", label: "Calibri" },
  { value: "Verdana", label: "Verdana" },
  { value: "Tahoma", label: "Tahoma" },
  { value: "Georgia", label: "Georgia (serif)" },
  { value: "Cambria", label: "Cambria (serif)" },
  { value: "Cascadia Code", label: "Cascadia Code (mono)" },
  { value: "Consolas", label: "Consolas (mono)" },
  { value: "JetBrains Mono", label: "JetBrains Mono (mono)" },
];

/** Curated base UI font sizes (px). 14 is the app default. */
const UI_FONT_SIZES: { value: number; label: string }[] = [
  { value: 12, label: "Small (12px)" },
  { value: 13, label: "13px" },
  { value: 14, label: "Default (14px)" },
  { value: 15, label: "15px" },
  { value: 16, label: "Large (16px)" },
  { value: 18, label: "Extra large (18px)" },
];

/** All reorderable/toggleable list columns. */
const COLUMN_CATALOG: { value: string; label: string }[] = [
  { value: "day", label: "Day" },
  { value: "time", label: "Time" },
  { value: "duration", label: "Duration" },
  { value: "title", label: "Title" },
  { value: "status", label: "Status" },
  { value: "tags", label: "Tags" },
  { value: "model", label: "Transcription Model" },
  { value: "cleanup_model", label: "Post-Processing Model" },
  { value: "summary_model", label: "Summary Model" },
  { value: "title_model", label: "Title Model" },
  { value: "tag_model", label: "Auto-Tag Model" },
  { value: "diarization_model", label: "Diarization Model" },
  { value: "diarized", label: "Diarized" },
  { value: "user_edited", label: "Edited" },
  { value: "source", label: "Source" },
  { value: "transcript", label: "Transcript Snippet" },
];

/**
 * Settings → Interface: the look-and-feel knobs under `config.interface` —
 * theme, 24h time, titlebar stripping, vim navigation, animation speed, step
 * notifications, quit semantics (`quit_stops_daemon`) — plus the recordings
 * list's column layout: a drag-to-reorder, toggleable list driving
 * `interface.visible_columns` (order = display order; see COLUMN_CATALOG),
 * with a reset-to-defaults action. Also hosts the "Reset interface
 * preferences" button that clears the per-device `phoneme.*` localStorage
 * keys. Plain section class on the form.ts binding; consumers apply changes
 * live off the `config:saved` event.
 */
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
        visible_columns: ["day", "time", "duration", "status", "source", "transcript"],
        quit_stops_daemon: true,
      };
    }

    const known = new Set(COLUMN_CATALOG.map((c) => c.value));
    const saved: string[] = (config.interface.visible_columns || []).filter((c: string) => known.has(c));
    // Visible columns first (saved order), then any remaining hidden columns.
    this.order = [
      ...saved,
      ...COLUMN_CATALOG.map((c) => c.value).filter((c) => !saved.includes(c)),
    ];
    // Transcript snippet is pinned as the last column — its read-more horizontal
    // scroll requires it, and any other position misbehaves. Keep it at the end.
    const pin = this.order.indexOf("transcript");
    if (pin >= 0) {
      this.order.splice(pin, 1);
      this.order.push("transcript");
    }
    this.visible = new Set(saved);

    this.renderShell();
    this.renderColumns();
  }

  private label(value: string): string {
    return COLUMN_CATALOG.find((c) => c.value === value)?.label ?? value;
  }

  /** Persist the current order + visibility into config and notify SettingsView. */
  private syncConfig() {
    const cols = this.order.filter((c) => this.visible.has(c));
    // Transcript snippet is always the last column (its read-more scroll
    // behavior requires it; any other position is buggy).
    const pin = cols.indexOf("transcript");
    if (pin >= 0 && pin !== cols.length - 1) {
      cols.splice(pin, 1);
      cols.push("transcript");
    }
    this.config.interface.visible_columns = cols;
    // Column widths are persisted per-column-NAME in localStorage (see
    // RecordingsList), so adding/removing/reordering columns no longer disturbs
    // them — nothing to clear here.
    // Bubbling change so SettingsView enables the Save button.
    this.container.dispatchEvent(new Event("change", { bubbles: true }));
  }

  private move(index: number, dir: -1 | 1) {
    const j = index + dir;
    if (j < 0 || j >= this.order.length) return;
    // Transcript is pinned last — never move it, and never move a column past it.
    if (this.order[index] === "transcript" || this.order[j] === "transcript") return;
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
              <input type="checkbox" class="col-toggle toggle-switch" value="${value}" ${this.visible.has(value) ? "checked" : ""} />
              <span>${this.label(value)}</span>
            </label>
            <span class="col-move">
              <button class="col-up" title="Move up" data-i="${i}" ${i === 0 || value === "transcript" ? "disabled" : ""}><svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 15 12 9 18 15"></polyline></svg></button>
              <button class="col-down" title="Move down" data-i="${i}" ${i === this.order.length - 1 || value === "transcript" || this.order[i + 1] === "transcript" ? "disabled" : ""}><svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>
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
          <label>Interface font</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ui-font">
              ${UI_FONT_CATALOG.map(
                (f) =>
                  `<option value="${f.value}" ${(config.interface.ui_font ?? "") === f.value ? "selected" : ""}>${f.label}</option>`,
              ).join("")}
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              The base typeface for the whole interface. Falls back to the bundled default if the
              font isn't installed. Transcript &amp; code blocks keep their fixed monospace font.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Interface font size</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ui-font-size">
              ${UI_FONT_SIZES.map(
                (s) =>
                  `<option value="${s.value}" ${Number(config.interface.ui_font_size ?? 14) === s.value ? "selected" : ""}>${s.label}</option>`,
              ).join("")}
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Base text size the rest of the UI scales from. Takes effect on save.
            </span>
          </div>
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

        <div class="settings-field">
          <label>Keyboard (vim) navigation</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "interface.vim_nav", label: "", kind: "checkbox" },
              config.interface.vim_nav,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              System-wide vim keys: <kbd>h</kbd>/<kbd>l</kbd> move focus between the sidebar, list, and
              detail panes; <kbd>j</kbd>/<kbd>k</kbd> and <kbd>gg</kbd>/<kbd>G</kbd> move within the list;
              <kbd>i</kbd>/<kbd>Enter</kbd> edit the transcript; <kbd>dd</kbd> deletes; <kbd>Esc</kbd> steps
              back out. Press <kbd>?</kbd> anytime for the full cheat-sheet. This is separate from the
              transcript editor's own vim mode (under Editor).
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Animation speed</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="anim-speed">
              <option value="off" ${config.interface.animation_speed === "off" ? "selected" : ""}>Off — instant</option>
              <option value="fast" ${config.interface.animation_speed === "fast" ? "selected" : ""}>Fast</option>
              <option value="normal" ${(config.interface.animation_speed ?? "normal") === "normal" ? "selected" : ""}>Normal</option>
              <option value="slow" ${config.interface.animation_speed === "slow" ? "selected" : ""}>Slow</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              How fast panes slide when shown/hidden (the sidebar <kbd>Ctrl+B</kbd>, the detail pane
              <kbd>Ctrl+\\</kbd>, and focus mode <kbd>f</kbd>). "Off" makes every toggle instant.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Step notifications</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "interface.step_notifications", label: "", kind: "checkbox" },
              config.interface.step_notifications ?? true,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Show a toast as each processing step finishes (transcribed, cleaned up, summarized,
              tags suggested) and when a recording is fully ready. Errors always notify, even
              with this off.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Quit stops the engine</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "interface.quit_stops_daemon", label: "", kind: "checkbox" },
              config.interface.quit_stops_daemon ?? true,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Quitting the tray also shuts down the background engine: an in-flight recording is
              finalized and queued first, and everything Phoneme started (whisper-server, an
              auto-launched Ollama) stops with it. Turn off to keep the engine running after the
              tray quits — hotkeyless/headless use. The OS-level tie to the tray's own death
              applies from the next engine start.
            </span>
          </div>
        </div>

        <div class="settings-field" style="align-items: flex-start;">
          <label style="margin-top: 8px;">Visible Columns</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 6px; width: 100%;">
            <div id="col-list" style="display: flex; flex-direction: column; gap: 8px;"></div>
            <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
              Check a column to show it; use the up/down chevrons to reorder. Columns appear left-to-right in this order. The transcript snippet is always shown last (it scrolls to read more inline).
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Reset remembered layout</label>
          <div><button class="inline-button" id="reset-ui-prefs" type="button">Reset interface preferences</button></div>
          <span style="grid-column: 2; font-size: 11px; color: var(--fg-faded);">
            Clears all per-device UI state remembered across reloads — column layout &amp; widths,
            panel split, sidebar, expanded meetings, the semantic-search toggle, record mode, and
            "don't ask again" prompts — back to defaults, then reloads.
          </span>
        </div>

      </div>
    `;
    bindFieldEvents(this.container, config);

    this.container
      .querySelector<HTMLSelectElement>("#anim-speed")
      ?.addEventListener("change", (e) => {
        config.interface.animation_speed = (e.target as HTMLSelectElement).value;
      });

    this.container
      .querySelector<HTMLSelectElement>("#ui-font")
      ?.addEventListener("change", (e) => {
        config.interface.ui_font = (e.target as HTMLSelectElement).value;
      });

    this.container
      .querySelector<HTMLSelectElement>("#ui-font-size")
      ?.addEventListener("change", (e) => {
        // Stored as a number — the Rust config field is a u8.
        config.interface.ui_font_size = Number((e.target as HTMLSelectElement).value);
      });

    this.container
      .querySelector<HTMLButtonElement>("#reset-ui-prefs")
      ?.addEventListener("click", () => void this.resetUiPrefs());
  }

  /** Clear every remembered per-device UI preference and reload. */
  private async resetUiPrefs() {
    const ok = confirm(
      "Reset all remembered interface preferences (column layout, panel sizes, expanded meetings, toggles, prompts)?\n\nThis reloads the app.",
    );
    if (!ok) return;
    // Per-device prefs live in localStorage under the "phoneme" prefix.
    try {
      Object.keys(localStorage)
        .filter((k) => k.startsWith("phoneme"))
        .forEach((k) => localStorage.removeItem(k));
    } catch {
      /* private mode — ignore */
    }
    // Column layout lives in config.toml — reset to defaults and persist.
    this.config.interface.visible_columns = [...DEFAULT_VISIBLE_COLUMNS];
    delete this.config.interface.column_widths;
    try {
      await invoke("write_config", { config: this.config });
    } catch {
      /* non-fatal — localStorage prefs are already cleared */
    }
    location.reload();
  }
}
