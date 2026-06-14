import { errText } from "../../utils/error";
import { updateNotes } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applyVimrc, defineVimWrite, VIM_SAVE_EVENT } from "../../utils/vimrc";
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
 *  - saves ONLY on an explicit action — the "Save Changes" button, Ctrl+S, or a
 *    vim `:w` / `:wq`. No auto-save on change or blur (it felt like the box was
 *    silently committing); unsaved edits are surfaced via the Save button and an
 *    `onDirtyChange` callback so the pane can prompt before they're abandoned.
 *  - respects the same `editor.vim_mode` / `editor.vimrc` config as the
 *    transcript editor so the user gets consistent keybindings everywhere
 */
export class NotesEditor {
  private container: HTMLElement;
  private id: string;
  private current: string;
  private lastSaved: string;
  private view: EditorView | null = null;
  private vimMode = false;
  private vimCurrentMode = "NORMAL";
  private vimBadgeElement: HTMLElement | null = null;
  private saveBtn: HTMLButtonElement | null = null;
  /** Copy button — a sibling of Save in the header row, shown only when clean
   *  (so it's never beside / overlapping Save Changes). */
  private copyBtn: HTMLButtonElement | null = null;
  private onDirtyChange?: (dirty: boolean) => void;
  private vimSaveHandler = () => {
    if (this.view?.hasFocus) void this.save();
  };

  constructor(container: HTMLElement, id: string, initial: string, onDirtyChange?: (dirty: boolean) => void) {
    this.container = container;
    this.id = id;
    this.current = initial;
    this.lastSaved = initial;
    this.onDirtyChange = onDirtyChange;
    void this.init();
  }

  /** Whether there are unsaved edits (used by the pane's leave-guard). */
  isDirty(): boolean {
    return this.current !== this.lastSaved;
  }

  /** The live editor text (what's on screen, including unsaved edits) — used by
   *  the notes-box Copy button so it copies what the user sees. */
  getText(): string {
    return this.current;
  }

  /** Copy the notes to the clipboard and flash a ✓ on the Copy button. */
  private async copyNotes(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.current);
      if (this.copyBtn) {
        this.copyBtn.textContent = "✅";
        this.copyBtn.style.color = "var(--ok)";
        this.copyBtn.style.borderColor = "var(--ok)";
        window.setTimeout(() => {
          if (!this.copyBtn) return;
          this.copyBtn.textContent = "📋";
          this.copyBtn.style.color = "var(--fg-muted)";
          this.copyBtn.style.borderColor = "var(--border-subtle)";
        }, 1500);
      }
    } catch (e) {
      showToast(`Clipboard copy failed: ${errText(e)}`, "error");
    }
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
        <button id="notes-copy-btn" title="Copy the notes to the clipboard" aria-label="Copy notes" style="display: inline-flex; align-items: center; justify-content: center; width: 26px; height: 24px; padding: 0; font-size: 13px; line-height: 1; border: 1px solid var(--border-subtle); border-radius: 4px; background: var(--bg-elevated); color: var(--fg-muted); cursor: pointer;">📋</button>
        <span style="flex: 1;"></span>
        <button id="notes-save-btn" style="display: none; background: var(--accent); color: var(--accent-fg); border: none; padding: 4px 10px; border-radius: 4px; font-size: 11px; cursor: pointer; font-weight: bold;">Save Changes</button>
      </div>
      <div id="notes-cm-root" class="notes-cm-root"></div>
    `;

    const root = this.container.querySelector<HTMLElement>("#notes-cm-root");
    if (!root) return;

    this.vimBadgeElement = this.container.querySelector<HTMLElement>("#notes-vim-badge");
    this.saveBtn = this.container.querySelector<HTMLButtonElement>("#notes-save-btn");
    this.saveBtn?.addEventListener("click", () => void this.save());
    this.copyBtn = this.container.querySelector<HTMLButtonElement>("#notes-copy-btn");
    this.copyBtn?.addEventListener("click", () => void this.copyNotes());

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
      // Align the first character with the "NOTES" header label above.
      ".cm-line": {
        paddingLeft: "0",
        paddingRight: "0",
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

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        this.current = update.state.doc.toString();
        this.updateSaveBtn();
        this.onDirtyChange?.(this.isDirty());
      }
    });

    // Ctrl/Cmd+S saves (mirrors the transcript editor). Deliberately no auto-save
    // on change or blur — the user commits explicitly via this, the Save button,
    // or a vim `:w`.
    const saveKeymap = keymap.of([
      { key: "Mod-s", run: () => { void this.save(); return true; } },
    ]);

    const extensions = [
      theme,
      EditorView.lineWrapping,
      updateListener,
      saveKeymap,
      drawSelection({ cursorBlinkRate: 1200 }),
      keymap.of(standardKeymap),
    ];

    if (vimMode) {
      extensions.unshift(vim());
      applyVimrc(vimrc, Vim);
      defineVimWrite(Vim);
    }

    this.view = new EditorView({
      state: EditorState.create({
        doc: this.current,
        extensions,
      }),
      parent: root,
    });

    document.addEventListener(VIM_SAVE_EVENT, this.vimSaveHandler);

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

  /** Show "Save Changes" only when there are unsaved edits; the Copy button is
   *  its inverse — visible only when clean, so the two never sit side by side. */
  private updateSaveBtn() {
    const dirty = this.current !== this.lastSaved;
    if (this.saveBtn) this.saveBtn.style.display = dirty ? "" : "none";
    if (this.copyBtn) this.copyBtn.style.display = dirty ? "none" : "inline-flex";
  }

  async save() {
    const value = this.current;
    if (value === this.lastSaved) return;
    try {
      await updateNotes(this.id, value);
      this.lastSaved = value;
      this.updateSaveBtn();
      this.onDirtyChange?.(false);
      showToast("Notes saved", "success");
    } catch (e) {
      showToast(`Failed to save notes: ${errText(e)}`, "error");
    }
  }

  dispose() {
    document.removeEventListener(VIM_SAVE_EVENT, this.vimSaveHandler);
    if (this.view) {
      this.view.destroy();
      this.view = null;
    }
  }
}
