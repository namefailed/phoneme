import { LitElement, html, css, PropertyValues } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';
import { updateTranscript } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applyVimrc } from "../../utils/vimrc";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { standardKeymap } from "@codemirror/commands";
import { vim, Vim } from "@replit/codemirror-vim";
import { invoke } from "@tauri-apps/api/core";

@customElement('ph-transcript-editor')
export class TranscriptEditorElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: String }) recordingId = "";
  @property({ type: String }) initialText = "";

  @state() private currentText = "";
  @state() private vimMode = false;
  @state() private vimCurrentMode = "NORMAL";

  @query('#cm-editor-root') editorRoot!: HTMLElement;

  private view: EditorView | null = null;

  connectedCallback() {
    super.connectedCallback();
    this.currentText = this.initialText;
    void this.initEditor();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.disposeEditor();
  }

  updated(changedProperties: PropertyValues) {
    if (changedProperties.has('initialText') && this.initialText !== changedProperties.get('initialText')) {
      if (!this.isDirty()) {
        this.currentText = this.initialText;
        if (this.view) {
          this.view.dispatch({
            changes: { from: 0, to: this.view.state.doc.length, insert: this.initialText }
          });
        }
      }
    }
  }

  private async initEditor() {
    let vimrc = "";
    let vimrcPath = "";
    try {
      const cfg = await invoke<any>("read_config");
      this.vimMode = cfg?.editor?.vim_mode || false;
      vimrc = cfg?.editor?.vimrc || "";
      vimrcPath = cfg?.editor?.vimrc_path || "";
    } catch (e) {
      console.error("Failed to load config for editor:", e);
    }

    if (vimrcPath) {
      try {
        const externalVimrc = await invoke<string>("read_file_string", { path: vimrcPath });
        vimrc = externalVimrc + "\n" + vimrc;
      } catch (e) {
        console.warn(`Failed to read external vimrc at ${vimrcPath}:`, e);
      }
    }

    this.mountEditor(vimrc);
  }

  private mountEditor(vimrc: string) {
    const theme = EditorView.theme({
      "&": {
        background: "transparent",
        color: "var(--fg-default)",
        height: "auto",
        minHeight: "150px",
        fontFamily: "inherit",
        fontSize: "14px",
      },
      ".cm-content": {
        caretColor: "var(--accent)",
        padding: "8px 0",
      },
      ".cm-cursor": {
        borderLeftColor: "var(--accent)"
      },
      "&.cm-focused": {
        outline: "none"
      },
      ".cm-activeLine": {
        backgroundColor: "rgba(255, 255, 255, 0.02)"
      },
      ".cm-activeLineGutter": {
        backgroundColor: "rgba(255, 255, 255, 0.02)"
      },
      ".cm-gutters": {
        display: "none"
      },
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
        backgroundColor: "color-mix(in srgb, var(--accent) 35%, transparent) !important",
      },
      ".cm-content ::selection": {
        backgroundColor: "color-mix(in srgb, var(--accent) 35%, transparent) !important",
      },
      ".cm-selectionMatch": {
        backgroundColor: "color-mix(in srgb, var(--accent) 25%, transparent) !important",
      },
      ".cm-fat-cursor": {
        backgroundColor: "color-mix(in srgb, var(--accent) 60%, transparent) !important",
        outline: "none !important",
      }
    });

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        this.currentText = update.state.doc.toString();
        this.dispatchEvent(new CustomEvent('dirty-change', { detail: this.isDirty() }));
      }
    });

    // Track vim mode changes using a custom extension
    const vimModeTracker = EditorView.domEventHandlers({
      keydown: (e) => {
        if (!this.vimMode) return;
        // Simple heuristic: if typing a character, we're in insert mode
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
          this.vimCurrentMode = "INSERT";
        } else if (e.key === "Escape") {
          this.vimCurrentMode = "NORMAL";
        }
      }
    });

    const extensions = [
      theme,
      EditorView.lineWrapping,
      updateListener,
      vimModeTracker,
      drawSelection({ cursorBlinkRate: 1200 }),
      keymap.of(standardKeymap),
    ];

    if (this.vimMode) {
      extensions.unshift(vim());
      applyVimrc(vimrc, Vim);
    }

    this.view = new EditorView({
      state: EditorState.create({
        doc: this.currentText,
        extensions,
      }),
      parent: this.editorRoot,
    });
  }

  private isDirty(): boolean {
    return this.currentText !== this.initialText;
  }

  public getText(): string {
    return this.currentText;
  }

  private disposeEditor() {
    if (this.view) {
      this.view.destroy();
      this.view = null;
    }
  }

  private handleKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === "s") {
      e.preventDefault();
      void this.save();
    }
  }

  async save() {
    if (!this.isDirty()) return;
    try {
      await updateTranscript(this.recordingId, this.currentText);
      this.initialText = this.currentText;
      this.dispatchEvent(new CustomEvent('dirty-change', { detail: false }));
      showToast("Transcript saved", "success");
    } catch (e) {
      showToast(`Failed to save transcript: ${e}`, "error");
    }
  }

  render() {
    return html`
      <style>
        ph-transcript-editor {
          display: block;
        }
        ph-transcript-editor .header {
          display: flex;
          justify-content: space-between;
          align-items: center;
          margin-bottom: 8px;
        }
        ph-transcript-editor .title {
          font-size: 11px;
          font-weight: bold;
          text-transform: uppercase;
          color: var(--fg-muted);
        }
        ph-transcript-editor .vim-badge {
          color: var(--accent);
          font-size: 9px;
          margin-left: 6px;
          border: 1px solid var(--accent);
          padding: 1px 4px;
          border-radius: 4px;
        }
        ph-transcript-editor .btn-save {
          background: var(--accent);
          color: var(--accent-fg);
          border: none;
          padding: 4px 10px;
          border-radius: 4px;
          font-size: 11px;
          cursor: pointer;
          font-weight: bold;
        }
      </style>
      <div class="header">
        <span class="title">
          Transcript ${this.vimMode ? html`<span class="vim-badge">${this.vimCurrentMode}</span>` : ""}
        </span>
        <button class="btn-save" style="display: ${this.isDirty() ? 'block' : 'none'};" @click=${this.save}>Save Changes</button>
      </div>
      <div id="cm-editor-root" @keydown=${this.handleKeydown}></div>
    `;
  }
}

// Temporary vanilla wrapper
export class TranscriptEditor {
  private element: TranscriptEditorElement;
  constructor(
    container: HTMLElement,
    id: string,
    initial: string,
    onDirtyChange: (dirty: boolean) => void,
  ) {
    this.element = document.createElement('ph-transcript-editor') as TranscriptEditorElement;
    this.element.recordingId = id;
    this.element.initialText = initial;
    this.element.addEventListener('dirty-change', (e: Event) => {
      onDirtyChange((e as CustomEvent<boolean>).detail);
    });
    container.appendChild(this.element);
  }

  getText(): string {
    return this.element.getText();
  }

  async save() {
    await this.element.save();
  }

  dispose() {
    this.element.remove();
  }
}
