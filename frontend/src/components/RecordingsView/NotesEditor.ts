import { errText } from "../../utils/error";
import { updateNotes } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applyVimrc, defineVimWrite, editorOwnsFocus, VIM_SAVE_EVENT } from "../../utils/vimrc";
import { openEditorMenu } from "./editorMenu";
import { loadCollapsed, saveCollapsed } from "./enrichSection";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { standardKeymap } from "@codemirror/commands";
import { vim, Vim, getCM } from "@replit/codemirror-vim";
import { invoke } from "@tauri-apps/api/core";

/** Right-pointing disclosure chevron (matches the Insights card + sidebar); the
 *  `.open` class rotates it to "down". An HTML string — NotesEditor renders via
 *  innerHTML, not Lit. */
const CHEVRON_SVG =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="9 6 15 12 9 18"></polyline></svg>';

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
  /** Section collapsed (remembered across reloads + recording switches), matching
   *  the Insights card's collapse pattern — only the editor folds; the bordered
   *  notes card keeps its header bar. */
  private collapsed = loadCollapsed("notes");
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

  /** Copy the notes to the clipboard (from the ⋯ menu). */
  private async copyNotes(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.current);
      showToast("Notes copied", "success");
    } catch (e) {
      showToast(`Clipboard copy failed: ${errText(e)}`, "error");
    }
  }

  /** Collapse / expand the notes editor (only the editor folds; the bordered
   *  notes card keeps its header bar — same pattern as the Insights card).
   *  Persisted per device. */
  private toggleCollapsed(btn: HTMLElement): void {
    this.collapsed = !this.collapsed;
    saveCollapsed("notes", this.collapsed);
    const wrap = this.container.querySelector<HTMLElement>(".notes-editor-wrap");
    if (wrap) wrap.style.display = this.collapsed ? "none" : "";
    btn.querySelector(".enrich-chevron")?.classList.toggle("open", !this.collapsed);
    btn.setAttribute("aria-expanded", String(!this.collapsed));
    btn.title = this.collapsed ? "Expand notes" : "Collapse notes";
    // CodeMirror measures zero height while hidden; re-measure on expand so the
    // text isn't clipped or misaligned on first show.
    if (!this.collapsed) this.view?.requestMeasure();
  }

  /** CodeMirror traps the wheel — when its own content fits (or you're already at
   *  a scroll boundary) the detail pane wouldn't scroll and you'd be stuck hovering
   *  the notes box. Forward the wheel to the detail pane in exactly those cases, so
   *  scrolling anywhere over the notes always scrolls the pane; let CM scroll its
   *  own content natively whenever it actually can. Mirrors the transcript editor. */
  private onNotesWheel = (e: WheelEvent) => {
    const detail = this.container.closest<HTMLElement>(".detail");
    if (!detail || detail.scrollHeight <= detail.clientHeight + 1) return;
    const sc = this.container.querySelector<HTMLElement>(".cm-scroller");
    if (sc && sc.scrollHeight > sc.clientHeight + 1 && sc.contains(e.target as Node)) {
      const atTop = sc.scrollTop <= 0;
      const atBottom = sc.scrollTop + sc.clientHeight >= sc.scrollHeight - 1;
      if ((e.deltaY < 0 && !atTop) || (e.deltaY > 0 && !atBottom)) return;
    }
    detail.scrollTop += e.deltaY;
    e.preventDefault();
  };

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
        <button id="notes-collapse-btn" class="enrich-toggle" aria-expanded="${!this.collapsed}" title="${this.collapsed ? "Expand notes" : "Collapse notes"}">
          <span class="enrich-chevron ${this.collapsed ? "" : "open"}">${CHEVRON_SVG}</span>
          <span class="enrich-label" style="font-weight: bold; color: var(--fg-muted);">Notes</span>
        </button>
        ${
          vimMode
            ? `<span id="notes-vim-badge" style="color: var(--accent); font-size: 0.6429rem; border: 1px solid var(--accent); padding: 1px 4px; border-radius: 4px;">NORMAL</span>`
            : ""
        }
        <span style="flex: 1;"></span>
        <button id="notes-save-btn" style="display: none; background: var(--accent); color: var(--accent-fg); border: none; padding: 4px 10px; border-radius: 4px; font-size: 0.7857rem; cursor: pointer; font-weight: bold;">Save Changes</button>
        <button id="notes-overflow-btn" class="editor-overflow-btn" title="More notes actions" aria-label="More notes actions" aria-haspopup="menu" aria-expanded="false">⋯</button>
      </div>
      <div class="notes-editor-wrap" ${this.collapsed ? 'style="display: none;"' : ""}>
        <div id="notes-cm-root" class="notes-cm-root"></div>
      </div>
    `;

    const root = this.container.querySelector<HTMLElement>("#notes-cm-root");
    if (!root) return;

    this.vimBadgeElement = this.container.querySelector<HTMLElement>("#notes-vim-badge");
    this.saveBtn = this.container.querySelector<HTMLButtonElement>("#notes-save-btn");
    this.saveBtn?.addEventListener("click", () => void this.save());
    const collapseBtn = this.container.querySelector<HTMLButtonElement>("#notes-collapse-btn");
    collapseBtn?.addEventListener("click", () => this.toggleCollapsed(collapseBtn));
    const overflowBtn = this.container.querySelector<HTMLButtonElement>("#notes-overflow-btn");
    overflowBtn?.addEventListener("click", () => {
      openEditorMenu(overflowBtn, [
        {
          label: "Copy notes",
          onSelect: () => void this.copyNotes(),
        },
        {
          label: "Find & Replace…",
          onSelect: async () => {
            const { openFindReplace } = await import("../FindReplace");
            await openFindReplace(this.id);
          },
        },
      ]);
    });

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

    // Forward the wheel to the detail pane when CM would otherwise trap it (its
    // content fits, or you're at a scroll boundary) — so hovering the notes box
    // never blocks scrolling the pane. Attached to the wrap so the overlaid Copy
    // button is covered too. Mirrors the transcript editor.
    this.container.querySelector<HTMLElement>(".notes-editor-wrap")
      ?.addEventListener("wheel", this.onNotesWheel, { passive: false });

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

  /** Show "Save Changes" only when there are unsaved edits. */
  private updateSaveBtn() {
    const dirty = this.current !== this.lastSaved;
    if (this.saveBtn) this.saveBtn.style.display = dirty ? "" : "none";
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
