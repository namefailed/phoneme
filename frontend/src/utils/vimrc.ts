/**
 * Vim-mode plumbing shared by the transcript and notes editors (CodeMirror +
 * @replit/codemirror-vim): global `:w`-family Ex commands, and a tiny .vimrc
 * mapping parser so users can bring their own keybindings (`editor.vimrc` /
 * `editor.vimrc_path` in config). Pure helpers — the editors own the
 * CodeMirror instances and pass the Vim API in.
 */

/** The slice of the codemirror-vim `Vim` API that `applyVimrc` drives —
 *  narrow on purpose so tests can pass a plain mock object. */
export interface VimMock {
  noremap: (keys: string, target: string, ctx: string) => void;
  map: (keys: string, target: string, ctx: string) => void;
}

/** Custom DOM event fired when the user runs `:w` (or `:wq`/`:x`) in a vim editor. */
export const VIM_SAVE_EVENT = "phoneme:vim-save";

let vimWriteDefined = false;
/**
 * Make `:w` / `:write` / `:wq` / `:x` save in any CodeMirror vim editor. The Ex
 * command is global to the Vim singleton, so we define it once and dispatch a
 * DOM event; the focused editor handles it and saves itself.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function defineVimWrite(Vim: any) {
  if (vimWriteDefined) return;
  // The event carries intent so the editor can save (`:w`), save-and-leave
  // (`:wq` / `:x`), or just leave (`:q`) — quitting hands focus back to the
  // pane nav, same as Shift+Esc.
  const fire = (save: boolean, quit: boolean) =>
    document.dispatchEvent(new CustomEvent(VIM_SAVE_EVENT, { detail: { save, quit } }));
  try {
    Vim.defineEx("write", "w", () => fire(true, false));
    Vim.defineEx("wq", "wq", () => fire(true, true));
    Vim.defineEx("xit", "x", () => fire(true, true));
    Vim.defineEx("quit", "q", () => fire(false, true));
    vimWriteDefined = true;
  } catch {
    /* older vim build without defineEx — silently skip */
  }
}

/**
 * Apply the map/noremap lines of a .vimrc to a Vim instance. Supported:
 * `[n|i|v](nore)map <keys> <target>` — the mode prefix picks the context
 * (none → normal), `nore` picks non-recursive. Everything else — comments
 * (`"`), blank lines, `set` options, and any line with fewer than three
 * fields — is silently skipped: this is a keybinding importer, not a vimrc
 * interpreter, and an unsupported line must never break the editor.
 */
export function applyVimrc(vimrc: string, vimInstance: VimMock) {
  if (!vimrc) return;
  const lines = vimrc.split("\n");
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('"')) continue;
    
    const parts = trimmed.split(/\s+/);
    if (parts.length < 3) continue;
    
    const cmd = parts[0];
    const keys = parts[1];
    const target = parts.slice(2).join(" ");
    
    const isInsert = cmd.startsWith("i");
    const isVisual = cmd.startsWith("v");
    const isNormal = cmd.startsWith("n");
    const isNoRemap = cmd.includes("noremap");
    
    let ctx = "normal";
    if (isInsert) ctx = "insert";
    else if (isNormal) ctx = "normal";
    else if (isVisual) ctx = "visual";
    
    if (isNoRemap) {
       vimInstance.noremap(keys, target, ctx);
    } else if (cmd.includes("map")) {
       vimInstance.map(keys, target, ctx);
    }
  }
}
