import { errText } from "../../utils/error";
import { updateNotes } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applyVimrc } from "../../utils/vimrc";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { standardKeymap } from "@codemirror/commands";
import { vim, Vim, getCM } from "@replit/codemirror-vim";
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
  private vimMode = false;
  private vimCurrentMode = "NORMAL";
  private vimBadgeElement: HTMLElement | null = null;

  constructor(container: HTMLElement, id: string, initial: string) {
    this.container = container;
    this.id = id;
    this.current = initial;
    this.lastSaved = initial;
    void this.init();
  }

  private async init() {
    let vimrc = "";
    let vimrcPath = "";
    try {
      const cfg = await invoke<any>("read_config");
      this.vimMode = cfg?.editor?.vim_mode || false;
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

    this.render(this.vimMode, vimrc);
  }

  private render(vimMode: boolean, vimrc: string) {
    this.container.innerHTML = `
      <div style="display: flex; align-items: center; margin-bottom: 4px; gap: 6px;">
        <label style="font-size: 11px; color: var(--fg-muted); font-weight: bold; text-transform: uppercase;">Notes</label>
        ${
          vimMode
            ? `<span id="notes-vim-badge" style="color: var(--accent); font-size: 9px; border: 1px solid var(--accent); padding: 1px 4px; border-radius: 4px;">NORMAL</span>`
            : ""
        }
      </div>
      <div id="notes-cm-root" class="notes-cm-root"></div>
    `;

    const root = this.container.querySelector<HTMLElement>("#notes-cm-root");
    if (!root) return;

    this.vimBadgeElement = this.container.querySelector<HTMLElement>("#notes-vim-badge");

    const theme = EditorView.theme({
      "&": {
        // Chrome-less: the surrounding `.notes-block` is the bordered box.
        background: "transparent",
        color: "var(--fg-default)",
        height: "auto",
        minHeight: "120px",
        fontFamily: "inherit",
        fontSize: "13px",
        border: "none",
        padding: "0",
      },
      ".cm-content": {
        caretColor: "var(--accent)",
        padding: "2px 0",
      },
      ".cm-cursor": {
        borderLeftColor: "var(--accent)",
      },
      "&.cm-focused": {
        outline: "none",
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

    // Reflect the REAL vim mode in the badge via the editor's own mode-change
    // events, not a keystroke heuristic. (No-op when vim mode is off.)
    if (vimMode) {
      const cm = getCM(this.view);
      cm?.on("vim-mode-change", (e: { mode?: string; subMode?: string }) => {
        const mode = (e?.mode ?? "normal").toUpperCase();
        this.vimCurrentMode = e?.subMode ? `${mode} ${e.subMode.toUpperCase()}` : mode;
        if (this.vimBadgeElement) this.vimBadgeElement.textContent = this.vimCurrentMode;
      });
    }
  }

  private async save() {
    const value = this.current;
    if (value === this.lastSaved) return;
    try {
      await updateNotes(this.id, value);
      this.lastSaved = value;
    } catch (e) {
      showToast(`Failed to save notes: ${errText(e)}`, "error");
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
