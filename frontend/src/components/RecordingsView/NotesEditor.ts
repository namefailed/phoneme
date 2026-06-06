import { updateNotes } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applyVimrc } from "../../utils/vimrc";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { standardKeymap } from "@codemirror/commands";
import { vim, Vim } from "@replit/codemirror-vim";
import { invoke } from "@tauri-apps/api/core";

export class NotesEditor {
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
    let vimrc = "";
    let vimrcPath = "";
    try {
      const cfg = await invoke<any>("read_config");
      vimMode = cfg?.editor?.vim_mode || false;
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

    this.render(vimMode, vimrc);
  }


  private render(vimMode: boolean, vimrc: string) {
    this.container.innerHTML = `
      <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px;">
        <span style="font-size: 11px; font-weight: bold; text-transform: uppercase; color: var(--fg-muted);">
          Notes ${vimMode ? '<span style="color: var(--accent); font-size: 9px; margin-left: 6px; border: 1px solid var(--accent); padding: 1px 4px; border-radius: 4px;">Vim Mode</span>' : ""}
        </span>
        <button id="btn-save-notes" style="display: none; background: var(--accent); color: var(--accent-fg); border: none; padding: 4px 10px; border-radius: 4px; font-size: 11px; cursor: pointer; font-weight: bold;">Save Changes</button>
      </div>
      <div id="cm-editor-root" class="cm-editor-root"></div>
    `;

    const editorRoot = this.container.querySelector<HTMLElement>("#cm-editor-root");
    const saveBtn = this.container.querySelector<HTMLButtonElement>("#btn-save-notes");
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
      // Selection backgrounds: CM renders .cm-selectionBackground divs when
      // drawSelection() is active. We must NOT use `opacity` on these divs
      // because that makes the div itself translucent, not just the color —
      // text inside the selected range becomes illegible. Use an rgba background
      // instead so the text layer above stays at full opacity.
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
        backgroundColor: "color-mix(in srgb, var(--accent) 35%, transparent) !important",
      },
      // Browser ::selection is a fallback for cases drawSelection misses.
      ".cm-content ::selection": {
        backgroundColor: "color-mix(in srgb, var(--accent) 35%, transparent) !important",
      },
      // Highlight search-match occurrences in the buffer.
      ".cm-selectionMatch": {
        backgroundColor: "color-mix(in srgb, var(--accent) 25%, transparent) !important",
      },
      // Vim block-cursor in normal mode.
      ".cm-fat-cursor": {
        backgroundColor: "color-mix(in srgb, var(--accent) 60%, transparent) !important",
        outline: "none !important",
      }
    });

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        this.current = update.state.doc.toString();
        this.onDirtyChange(this.current !== this.initial);
        updateSaveBtn();
      }
    });

    // drawSelection() must always be present — it replaces the browser's
    // native ::selection with CM-managed highlight divs (.cm-selectionBackground).
    // Without it, vim visual mode (v, V) and mouse selections produce no visible
    // highlight because the browser suppresses ::selection inside a shadow DOM.
    // It must be listed BEFORE vim() so the vim extension can see it.
    const extensions = [
      theme,
      EditorView.lineWrapping,
      updateListener,
      drawSelection({ cursorBlinkRate: 1200 }),
      keymap.of(standardKeymap),
    ];

    if (vimMode) {
      extensions.unshift(vim());
      applyVimrc(vimrc, Vim);
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
    try {
      await updateNotes(this.id, this.current);
      this.initial = this.current;
      this.onDirtyChange(false);
      const saveBtn = this.container.querySelector<HTMLButtonElement>("#btn-save-notes");
      if (saveBtn) saveBtn.style.display = "none";
      showToast("Notes saved", "success");
    } catch (e) {
      showToast(`Failed to save notes: ${e}`, "error");
    }
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
