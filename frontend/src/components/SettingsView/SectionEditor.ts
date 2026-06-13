import { bindFieldEvents, renderField } from "./form";

/**
 * Settings → Editor: the transcript/notes editors' vim mode
 * (`editor.vim_mode`), an external .vimrc path with a Browse picker
 * (`editor.vimrc_path`), and an inline vimrc text block (`editor.vimrc`).
 * Both CodeMirror editors read these on mount (see utils/vimrc.ts for what
 * the vimrc parser supports). Plain section class on the form.ts binding.
 */
export class SectionEditor {
  private config: any;

  constructor(container: HTMLElement, config: any) {
    this.config = config;
    this.render(container);
  }

  private render(container: HTMLElement) {
    if (!this.config.editor) {
      this.config.editor = { vim_mode: false, vimrc: "", vimrc_path: "" };
    }

    container.innerHTML = `
      <div class="settings-section">
        <h3>Editor Settings</h3>
        
        <div class="settings-field">
          <label>Vim keybindings in Editor</label>
          <div>${renderField(
            { key: "editor.vim_mode", label: "", kind: "checkbox" },
            this.config.editor.vim_mode || false,
          )}</div>
        </div>

        <div class="settings-field" style="flex-direction: column; align-items: flex-start; gap: 8px;">
          <label>External Vimrc Path (Optional)</label>
          <div style="display: flex; gap: 8px; width: 100%;">
            ${renderField(
              { key: "editor.vimrc_path", label: "", kind: "text" },
              this.config.editor.vimrc_path || "",
            )}
            <button class="inline-button" id="pick-vimrc" style="white-space: nowrap;">Browse…</button>
          </div>
          <span style="font-size: 11px; color: var(--fg-faded); line-height: 1.4;">
            Absolute path to a <code>.vimrc</code> file on your computer (e.g., <code>~/.vimrc</code> or <code>C:\\Users\\Namef\\.vimrc</code>). Phoneme will read and apply these mappings automatically.
          </span>
        </div>

        <div class="settings-field" style="flex-direction: column; align-items: flex-start; gap: 8px;">
          <label>Vimrc Configurations (Inline)</label>
          <div style="width: 100%;">${renderField(
            { key: "editor.vimrc", label: "", kind: "textarea" },
            this.config.editor.vimrc || "",
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); line-height: 1.4;">
            Map custom keybindings for Vim mode (e.g., <code>imap jj &lt;Esc&gt;</code>, <code>nnoremap &lt;C-c&gt; yy</code>). Note: CodeMirror Vim is an emulation layer, so advanced plugins won't work.
          </span>
        </div>
      </div>
    `;

    bindFieldEvents(container, this.config);

    container.querySelector("#pick-vimrc")?.addEventListener("click", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({
        multiple: false,
      });
      if (typeof path === "string") {
        const input = container.querySelector<HTMLInputElement>(
          `[data-key="editor.vimrc_path"]`,
        )!;
        input.value = path;
        this.config.editor.vimrc_path = path;
      }
    });
  }
}
