import { updateNotes } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applyVimrc } from "../../utils/vimrc";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { standardKeymap } from "@codemirror/commands";
import { vim, Vim } from "@replit/codemirror-vim";
import { invoke } from "@tauri-apps/api/core";

/**
 * A CodeMirror-backed editor for the per-recording Notes field.
 *
 * Mirrors TranscriptEditor but:
 *  - saves via `updateNotes` (not `updateTranscript`)
 *  - auto-saves on change (debounced 800 ms) and on blur — no explicit
 *    "Save Changes" button, matching the previous textarea UX
 *  - respects the same `editor.vim_mode` / `editor.vimrc` config as the
 *    transcript editor so the user gets consistent keybindings everywhere
 */
export class NotesEditor {
  private container: HTMLElement;
  private id: string;
  private current: string;
  private lastSaved: string;
  private view: EditorView | null = null;
  private debounce: ReturnType<typeof setTimeout> | undefined;

  constructor(container: HTMLElement, id: string, initial: string) {
    this.container = container;
    this.id = id;
    this.current = initial;
    this.lastSaved = initial;
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
      console.error("NotesEditor: failed to load config:", e);
    }

    if (vimrcPath) {
      try {
        const external = await invoke<string>("read_file_string", {
          path: vimrcPath,
        });
        vimrc = external + "\n" + vimrc;
      } catch (e) {
        console.warn(`NotesEditor: could not read vimrc at ${vimrcPath}:`, e);
      }
    }

    this.render(vimMode, vimrc);
  }

  private render(vimMode: boolean, vimrc: string) {
    this.container.innerHTML = `
      <div style="display: flex; align-items: center; margin-bottom: 4px; gap: 6px;">
        <label style="font-size: 11px; color: var(--fg-muted); font-weight: bold; text-transform: uppercase;">Notes</label>
        ${
          vimMode
            ? `<span style="color: var(--accent); font-size: 9px; border: 1px solid var(--accent); padding: 1px 4px; border-radius: 4px;">Vim Mode</span>`
            : ""
        }
      </div>
      <div id="notes-cm-root" class="notes-cm-root"></div>
    `;

    const root = this.container.querySelector<HTMLElement>("#notes-cm-root");
    if (!root) return;

    const theme = EditorView.theme({
      "&": {
        background: "var(--bg-input, var(--bg-subtle))",
        color: "var(--fg-default)",
        height: "auto",
        minHeight: "72px",
        fontFamily: "inherit",
        fontSize: "13px",
        borderRadius: "6px",
        border: "1px solid var(--border-subtle)",
        padding: "4px 2px",
      },
      ".cm-content": {
        caretColor: "var(--accent)",
        padding: "4px 8px",
      },
      ".cm-cursor": {
        borderLeftColor: "var(--accent)",
      },
      "&.cm-focused": {
        outline: "none",
        borderColor: "var(--accent)",
      },
      ".cm-activeLine": {
        backgroundColor: "rgba(255,255,255,0.02)",
      },
      ".cm-gutters": {
        display: "none",
      },
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
        backgroundColor:
          "color-mix(in srgb, var(--accent) 35%, transparent) !important",
      },
      ".cm-content ::selection": {
        backgroundColor:
          "color-mix(in srgb, var(--accent) 35%, transparent) !important",
      },
      ".cm-fat-cursor": {
        backgroundColor:
          "color-mix(in srgb, var(--accent) 60%, transparent) !important",
        outline: "none !important",
      },
    });

    const saveHandler = () => {
      if (this.debounce) clearTimeout(this.debounce);
      this.debounce = setTimeout(() => void this.save(), 800);
    };

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        this.current = update.state.doc.toString();
        saveHandler();
      }
    });

    // Blur-on-focusout: flush the debounce immediately.
    const blurListener = EditorView.domEventHandlers({
      blur: () => {
        if (this.debounce) clearTimeout(this.debounce);
        void this.save();
        return false;
      },
    });

    const extensions = [
      theme,
      EditorView.lineWrapping,
      updateListener,
      blurListener,
      drawSelection({ cursorBlinkRate: 1200 }),
      keymap.of(standardKeymap),
    ];

    if (vimMode) {
      extensions.unshift(vim());
      applyVimrc(vimrc, Vim);
    }

    this.view = new EditorView({
      state: EditorState.create({
        doc: this.current,
        extensions,
      }),
      parent: root,
    });
  }

  private async save() {
    const value = this.current;
    if (value === this.lastSaved) return;
    try {
      await updateNotes(this.id, value);
      this.lastSaved = value;
    } catch (e) {
      showToast(`Failed to save notes: ${String(e)}`, "error");
    }
  }

  dispose() {
    if (this.debounce) clearTimeout(this.debounce);
    if (this.view) {
      this.view.destroy();
      this.view = null;
    }
  }
}
