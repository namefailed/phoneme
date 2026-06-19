import { errText } from "../../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';
import { updateTranscript } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applyVimrc, defineVimWrite, editorOwnsFocus, VIM_SAVE_EVENT } from "../../utils/vimrc";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { standardKeymap, history, historyKeymap } from "@codemirror/commands";
import { vim, Vim, getCM } from "@replit/codemirror-vim";
import { invoke } from "@tauri-apps/api/core";

/**
 * The transcript editor: a CodeMirror 6 instance over the recording's live
 * transcript, with optional vim keybindings (`editor.vim_mode` config, plus
 * the user's `editor.vimrc`/`vimrc_path` mappings and a mode badge).
 *
 * Save model: explicit only — the "Save Changes" button, Ctrl+S, or vim
 * `:w`/`:wq` (the global VIM_SAVE_EVENT, answered only by the focused
 * editor). Saving calls `updateTranscript` (the daemon preserves the machine
 * original separately and broadcasts `transcript_updated`). Dirtiness is
 * reported via the `dirty-change` CustomEvent — RecordingDetail uses it for
 * the unsaved-edits guards — and a refresh with new upstream text only
 * replaces the buffer when the editor is CLEAN, so live pipeline updates
 * never clobber typing.
 *
 * Keyboard: Shift+Esc (and vim `:q`) leave the editor back to the pane nav
 * (`phoneme:vim` "exit-editor"); plain Esc stays inside (vim mode needs it).
 */
@customElement('ph-transcript-editor')
export class TranscriptEditorElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: String }) recordingId = "";
  @property({ type: String }) initialText = "";
  /** Whether this transcript was manually edited before (the catalog's
   *  `user_edited` flag) — surfaced as an "Edited" badge next to Save. */
  @property({ type: Boolean }) userEdited = false;
  /** Transform applied to the editor text before copying (the host uses it to
   *  bake in custom speaker names). Identity when unset. */
  @property({ attribute: false }) copyTransform?: (text: string) => string;

  @state() private currentText = "";
  @state() private vimMode = false;
  @state() private vimCurrentMode = "NORMAL";
  /** Brief ✓ flash on the Copy button right after a successful copy. */
  @state() private copied = false;

  @query('#cm-editor-root') editorRoot!: HTMLElement;

  private view: EditorView | null = null;

  private vimSaveHandler = (e: Event) => {
    // Only the focused editor responds to a global `:w` / `:wq` / `:q` — focus
    // counts whether it's in the content or this editor's `:` dialog (the dialog
    // holds focus while the command fires, so `hasFocus` alone misses it).
    if (!editorOwnsFocus(this.view)) return;
    const detail = (e as CustomEvent)?.detail ?? {};
    const save = detail.save !== false; // default true for a plain `:w`
    const quit = !!detail.quit;
    // Saving (when dirty) clears the "Save Changes" button via the re-render;
    // a quit (`:wq` / `:q`) then leaves the editor back to the pane nav.
    const leave = () => {
      if (quit) window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "exit-editor" } }));
    };
    if (save) void this.save().then(leave);
    else leave();
  };

  connectedCallback() {
    super.connectedCallback();
    this.currentText = this.initialText;
    void this.initEditor();
    document.addEventListener(VIM_SAVE_EVENT, this.vimSaveHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.disposeEditor();
    document.removeEventListener(VIM_SAVE_EVENT, this.vimSaveHandler);
  }

  firstUpdated() {
    // Forward wheel events from the overlaid Copy button to the transcript's own
    // scroller. The button is position:absolute OUTSIDE `.cm-scroller`, so its
    // native scroll target is the outer pane — meaning a wheel over the button at
    // the editor's top corner would scroll the wrong thing (or nothing when only
    // the transcript itself scrolls), a dead zone the user hit. Forwarding makes
    // hovering the button behave like hovering the text.
    this.querySelector<HTMLElement>(".btn-copy")
      ?.addEventListener("wheel", this.forwardCopyWheel, { passive: false });
  }

  private forwardCopyWheel = (e: WheelEvent) => {
    const sc = this.querySelector<HTMLElement>(".cm-scroller");
    if (sc && sc.scrollHeight > sc.clientHeight + 1) {
      sc.scrollTop += e.deltaY;
      e.preventDefault();
    }
    // else: let the event bubble so the outer pane scrolls naturally.
  };

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

    // Ensure the host has rendered so #cm-editor-root exists before CodeMirror
    // attaches to it. The read_config await above is normally slow enough (an IPC
    // round-trip) that the first render already happened, but a fast/synchronous
    // config (a cached or mocked backend) can resolve before the first paint —
    // which left CodeMirror parented to a missing node, i.e. an empty box.
    await this.updateComplete;
    this.mountEditor(vimrc);
  }

  private mountEditor(vimrc: string) {
    const theme = EditorView.theme({
      "&": {
        // Chrome-less: the surrounding `.transcript-block` is the bordered box,
        // so the editor itself is transparent with no border/radius/padding.
        // This avoids the "box-in-a-box" double border + doubled left padding
        // that pushed the text far from the edge. height:100% fills the block.
        background: "transparent",
        color: "var(--fg-default)",
        height: "100%",
        minHeight: "150px",
        fontFamily: "inherit",
        fontSize: "1rem",
        border: "none",
        padding: "0",
      },
      ".cm-content": {
        caretColor: "var(--accent)",
        // Minimal padding — horizontal alignment comes from the block's padding.
        padding: "2px 0",
      },
      // Zero CodeMirror's default 2px line padding so the first character lines
      // up exactly with the "TRANSCRIPT" header label above it.
      ".cm-line": {
        paddingLeft: "0",
        paddingRight: "0",
      },
      ".cm-cursor": {
        borderLeftColor: "var(--accent)"
      },
      "&.cm-focused": {
        outline: "none",
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

    const extensions = [
      theme,
      // REQUIRED for undo/redo to exist at all — without the history state field,
      // both vim's `u` (normal mode) and Ctrl+Z call `undo` against nothing.
      history(),
      EditorView.lineWrapping,
      updateListener,
      drawSelection({ cursorBlinkRate: 1200 }),
      keymap.of([...historyKeymap, ...standardKeymap]),
    ];

    if (this.vimMode) {
      extensions.unshift(vim());
      applyVimrc(vimrc, Vim);
      defineVimWrite(Vim);
    }

    this.view = new EditorView({
      state: EditorState.create({
        doc: this.currentText,
        extensions,
      }),
      parent: this.editorRoot,
    });

    // Reflect the REAL vim mode in the badge by subscribing to the editor's
    // own mode-change events, rather than guessing from keystrokes. The legacy
    // CodeMirror adapter from `getCM` emits "vim-mode-change" with the actual
    // mode ("normal" | "insert" | "visual" | ...).
    if (this.vimMode) {
      const cm = getCM(this.view);
      cm?.on("vim-mode-change", (e: { mode?: string; subMode?: string }) => {
        const mode = (e?.mode ?? "normal").toUpperCase();
        // Distinguish visual sub-modes (e.g. VISUAL LINE / VISUAL BLOCK).
        this.vimCurrentMode = e?.subMode ? `${mode} ${e.subMode.toUpperCase()}` : mode;
        this.requestUpdate();
      });
    }
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
      return;
    }
    // Shift+Esc leaves the editor and hands focus back to the keyboard-nav layer
    // (the detail pane), so h/l/j/k work again. Plain Esc can't do this here —
    // it's bound to the editor's own vim normal mode.
    if (e.shiftKey && e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "exit-editor" } }));
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
      showToast(`Failed to save transcript: ${errText(e)}`, "error");
    }
  }

  /** Copy the transcript (with the host's transform — speaker names — applied)
   *  and flash a ✓. The button is only shown when clean, so this never fights
   *  the Save Changes button. */
  private async requestCopy() {
    const text = this.copyTransform ? this.copyTransform(this.currentText) : this.currentText;
    try {
      await navigator.clipboard.writeText(text);
      this.copied = true;
      window.setTimeout(() => { this.copied = false; }, 1500);
    } catch (e) {
      showToast(`Clipboard copy failed: ${errText(e)}`, "error");
    }
  }

  render() {
    return html`
      <style>
        ph-transcript-editor {
          display: flex;
          flex-direction: column;
          flex: 1;
          min-height: 0;
        }
        ph-transcript-editor #cm-editor-root {
          flex: 1;
          display: flex;
          flex-direction: column;
          min-height: 0;
        }
        ph-transcript-editor #cm-editor-root .cm-editor {
          flex: 1;
        }
        /* Relative wrapper so the Copy button can overlay the text top-right. */
        ph-transcript-editor .editor-wrap {
          position: relative;
          flex: 1;
          display: flex;
          flex-direction: column;
          min-height: 0;
        }
        ph-transcript-editor .header {
          display: flex;
          justify-content: flex-start;
          gap: 8px;
          align-items: center;
          margin-bottom: 8px;
          flex: 0 0 auto;
        }
        /* Spacer pushes the Edited/Save actions to the right of the title. */
        ph-transcript-editor .header-spacer { flex: 1; }
        ph-transcript-editor .title {
          font-size: 0.7857rem;
          font-weight: bold;
          text-transform: uppercase;
          color: var(--fg-muted);
        }
        ph-transcript-editor .vim-badge {
          color: var(--accent);
          font-size: 0.6429rem;
          margin-left: 6px;
          border: 1px solid var(--accent);
          padding: 1px 4px;
          border-radius: 4px;
        }
        ph-transcript-editor .header-actions {
          display: flex;
          align-items: center;
          gap: 8px;
        }
        ph-transcript-editor .btn-save {
          background: var(--accent);
          color: var(--accent-fg);
          border: none;
          padding: 4px 10px;
          border-radius: 4px;
          font-size: 0.7857rem;
          cursor: pointer;
          font-weight: bold;
        }
        /* Copy is a bare icon tucked into the text's top corner, web-page style.
           Very faded at rest; HIDDEN while the box is focused (actually editing)
           so it never sits over the text you're typing — but hovering the text
           always reveals it, fully lit, even mid-edit. No button chrome. */
        ph-transcript-editor .btn-copy {
          position: absolute;
          top: 4px;
          right: 6px;
          z-index: 5;
          display: inline-flex;
          align-items: center;
          justify-content: center;
          width: 24px;
          height: 24px;
          padding: 0;
          font-size: 1rem;
          line-height: 1;
          border: none;
          background: transparent;
          box-shadow: none;
          color: var(--fg-muted);
          cursor: pointer;
          opacity: 0.2;
          transition: opacity 0.15s ease, color 0.15s ease;
        }
        ph-transcript-editor .editor-wrap:focus-within .btn-copy { opacity: 0; }
        ph-transcript-editor .editor-wrap:hover .btn-copy,
        ph-transcript-editor .editor-wrap .btn-copy:hover,
        ph-transcript-editor .editor-wrap .btn-copy:focus-visible,
        ph-transcript-editor .editor-wrap .btn-copy.kbd-cursor,
        ph-transcript-editor .editor-wrap .btn-copy.copied {
          opacity: 1;
        }
        ph-transcript-editor .editor-wrap:hover .btn-copy,
        ph-transcript-editor .editor-wrap .btn-copy:hover { color: var(--accent); }
        /* "Edited" status badge — same footprint as Save, but a non-interactive
           accent-tinted pill so it reads as a marker, not an action. */
        ph-transcript-editor .edited-badge {
          padding: 4px 10px;
          border-radius: 4px;
          font-size: 0.7857rem;
          font-weight: bold;
          background: color-mix(in srgb, var(--accent) 16%, transparent);
          color: var(--accent);
          border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
        }
      </style>
      <div class="header">
        <span class="title">
          Transcript ${this.vimMode ? html`<span class="vim-badge">${this.vimCurrentMode}</span>` : ""}
        </span>
        <span class="header-spacer"></span>
        <div class="header-actions">
          ${this.userEdited ? html`<span class="edited-badge" title="This transcript has been manually edited">✓ Edited</span>` : ""}
          <button class="btn-save" style="display: ${this.isDirty() ? 'inline-flex' : 'none'};" @click=${this.save}>Save Changes</button>
        </div>
      </div>
      <div class="editor-wrap">
        <button
          class="btn-copy ${this.copied ? "copied" : ""}"
          title="Copy the transcript to the clipboard"
          aria-label="Copy transcript"
          @click=${this.requestCopy}
        >${this.copied ? "✅" : "📋"}</button>
        <div id="cm-editor-root" @keydown=${this.handleKeydown}></div>
      </div>
    `;
  }
}

/** Imperative mount wrapper (RecordingDetail's handle on the editor). */
export class TranscriptEditor {
  private element: TranscriptEditorElement;
  /** Mounts `<ph-transcript-editor>` into `container` and adapts the
   *  `dirty-change` event to the `onDirtyChange` callback RecordingDetail
   *  passes. `dispose()` unmounts (CodeMirror tears down with the element). */
  constructor(
    container: HTMLElement,
    id: string,
    initial: string,
    onDirtyChange: (dirty: boolean) => void,
    userEdited = false,
    copyTransform?: (text: string) => string,
  ) {
    this.element = document.createElement('ph-transcript-editor') as TranscriptEditorElement;
    this.element.recordingId = id;
    this.element.initialText = initial;
    this.element.userEdited = userEdited;
    this.element.copyTransform = copyTransform;
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
