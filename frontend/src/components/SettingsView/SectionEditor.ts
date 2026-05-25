import { bindFieldEvents, renderField } from "./form";

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
          <div style="width: 100%;">${renderField(
            { key: "editor.vimrc_path", label: "", kind: "text" },
            this.config.editor.vimrc_path || "",
          )}</div>
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
  }
}
