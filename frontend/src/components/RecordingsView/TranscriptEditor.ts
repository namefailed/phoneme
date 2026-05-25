import { updateTranscript } from "../../services/ipc";
import { EditorView, keymap } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { standardKeymap } from "@codemirror/commands";
import { vim } from "@replit/codemirror-vim";
import { invoke } from "@tauri-apps/api/core";

export class TranscriptEditor {
  private container: HTMLElement;
  private id: string;
  private initial: string;
  private current: string;
  private onDirtyChange: (dirty: boolean) => void;
  private view: EditorView | null = null;

  constructor(
    container: HTMLElement,
    id: string,
    initial: string,
    onDirtyChange: (dirty: boolean) => void,
  ) {
    this.container = container;
    this.id = id;
    this.initial = initial;
    this.current = initial;
    this.onDirtyChange = onDirtyChange;
    void this.init();
  }

  private async init() {
    let vimMode = false;
    try {
      const cfg = await invoke<any>("read_config");
      vimMode = cfg?.tray?.vim_mode || false;
    } catch (e) {
      console.error("Failed to load config for editor:", e);
    }

    this.render(vimMode);
  }

  private render(vimMode: boolean) {
    this.container.innerHTML = `
      <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px;">
        <span style="font-size: 11px; font-weight: bold; text-transform: uppercase; color: var(--fg-muted);">
          Transcript ${vimMode ? '<span style="color: var(--accent); font-size: 9px; margin-left: 6px; border: 1px solid var(--accent); padding: 1px 4px; border-radius: 4px;">Vim Mode</span>' : ""}
        </span>
        <button id="btn-save-transcript" style="display: none; background: var(--accent); color: var(--accent-fg); border: none; padding: 4px 10px; border-radius: 4px; font-size: 11px; cursor: pointer; font-weight: bold;">Save Changes</button>
      </div>
      <div id="cm-editor-root" class="cm-editor-root"></div>
    `;

    const editorRoot = this.container.querySelector<HTMLElement>("#cm-editor-root");
    const saveBtn = this.container.querySelector<HTMLButtonElement>("#btn-save-transcript");
    if (!editorRoot) return;

    const updateSaveBtn = () => {
      if (saveBtn) {
        saveBtn.style.display = this.current !== this.initial ? "block" : "none";
      }
    };

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
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
        backgroundColor: "rgba(255, 255, 255, 0.15) !important"
      },
      ".cm-fat-cursor": {
        backgroundColor: "var(--accent) !important",
        color: "var(--accent-fg) !important"
      }
    });

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        this.current = update.state.doc.toString();
        this.onDirtyChange(this.current !== this.initial);
        updateSaveBtn();
      }
    });

    const extensions = [
      theme,
      EditorView.lineWrapping,
      updateListener,
      keymap.of(standardKeymap),
    ];

    if (vimMode) {
      extensions.push(vim());
    }

    this.view = new EditorView({
      state: EditorState.create({
        doc: this.initial,
        extensions,
      }),
      parent: editorRoot,
    });

    editorRoot.addEventListener("keydown", (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s") {
        e.preventDefault();
        void this.save();
      }
    });

    if (saveBtn) {
      saveBtn.addEventListener("click", () => {
        void this.save();
      });
    }
  }

  async save() {
    if (this.current === this.initial) return;
    await updateTranscript(this.id, this.current);
    this.initial = this.current;
    this.onDirtyChange(false);
    const saveBtn = this.container.querySelector<HTMLButtonElement>("#btn-save-transcript");
    if (saveBtn) saveBtn.style.display = "none";
  }

  getText(): string {
    return this.current;
  }

  dispose() {
    if (this.view) {
      this.view.destroy();
      this.view = null;
    }
  }
}
