import { errText } from "../../utils/error";
import { updateNotes } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applyVimrc, defineVimWrite, editorOwnsFocus, VIM_SAVE_EVENT } from "../../utils/vimrc";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { standardKeymap } from "@codemirror/commands";
import { vim, Vim, getCM } from "@replit/codemirror-vim";
import { invoke } from "@tauri-apps/api/core";

/** Monochrome copy / check glyphs (currentColor) — shared shape with the
 *  transcript editor's Copy button. Inline SVG, never the tofu-prone clipboard
 *  emoji. */
const COPY_SVG =
  '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
const CHECK_SVG =
  '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="20 6 9 17 4 12"/></svg>';

/**
 * A CodeMirror-backed editor for the per-recording Notes field.
 *
 * Mirrors TranscriptEditor but:
 *  - saves via `updateNotes` (not `updateTranscript`)
 *  - saves only on an explicit action — the "Save Changes" button, Ctrl+S, or a
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
    // Save when focus is in the content or this editor's `:` dialog (the dialog
    // holds focus while `:w` runs, so `hasFocus` alone would miss it).
    if (editorOwnsFocus(this.view)) void this.save();
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
        this.copyBtn.classList.add("copied");
        this.copyBtn.innerHTML = CHECK_SVG;
        window.setTimeout(() => {
          if (!this.copyBtn) return;
          this.copyBtn.classList.remove("copied");
          this.copyBtn.innerHTML = COPY_SVG;
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
        <label style="font-size: 0.7857rem; color: var(--fg-muted); font-weight: bold; text-transform: uppercase;">Notes</label>
        ${
          vimMode
            ? `<span id="notes-vim-badge" style="color: var(--accent); font-size: 0.6429rem; border: 1px solid var(--accent); padding: 1px 4px; border-radius: 4px;">NORMAL</span>`
            : ""
        }
        <span style="flex: 1;"></span>
        <button id="notes-save-btn" style="display: none; background: var(--accent); color: var(--accent-fg); border: none; padding: 4px 10px; border-radius: 4px; font-size: 0.7857rem; cursor: pointer; font-weight: bold;">Save Changes</button>
      </div>
      <div class="notes-editor-wrap">
        <button id="notes-copy-btn" class="notes-copy-overlay" title="Copy the notes to the clipboard" aria-label="Copy notes">${COPY_SVG}</button>
        <div id="notes-cm-root" class="notes-cm-root"></div>
      </div>
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
        fontSize: "0.9286rem",
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

    // Shift+Esc leaves the editor and hands focus back to the keyboard-nav layer
    // (the detail pane), so h/l/j/k work again — exactly like the transcript
    // editor. Plain Esc is the editor's own vim normal mode here, so Shift is the
    // explicit "leave the box" gesture. The event bubbles from the CodeMirror
    // content up to this root (Shift+Esc isn't a vim binding, so vim lets it pass).
    root.addEventListener("keydown", (e) => {
      if (e.shiftKey && e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "exit-editor" } }));
      }
    });

    document.addEventListener(VIM_SAVE_EVENT, this.vimSaveHandler);

    // Drive the badge from the editor's own mode-change events — the actual vim
    // mode, not a keystroke heuristic. (No-op when vim mode is off.)
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
