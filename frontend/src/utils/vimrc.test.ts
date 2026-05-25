import { describe, it, expect, vi } from "vitest";
import { applyVimrc, type VimMock } from "./vimrc";

describe("applyVimrc", () => {
  it("ignores empty lines and comments", () => {
    const mockVim: VimMock = { noremap: vi.fn(), map: vi.fn() };
    const vimrc = `
    
      " This is a comment
      "nmap j k
    `;
    applyVimrc(vimrc, mockVim);
    expect(mockVim.noremap).not.toHaveBeenCalled();
    expect(mockVim.map).not.toHaveBeenCalled();
  });

  it("handles basic mappings", () => {
    const mockVim: VimMock = { noremap: vi.fn(), map: vi.fn() };
    const vimrc = `
      nmap j gj
      vmap k gk
      imap jj <Esc>
    `;
    applyVimrc(vimrc, mockVim);
    expect(mockVim.map).toHaveBeenCalledWith("j", "gj", "normal");
    expect(mockVim.map).toHaveBeenCalledWith("k", "gk", "visual");
    expect(mockVim.map).toHaveBeenCalledWith("jj", "<Esc>", "insert");
  });

  it("handles noremap mappings", () => {
    const mockVim: VimMock = { noremap: vi.fn(), map: vi.fn() };
    const vimrc = `
      nnoremap j gj
      vnoremap k gk
      inoremap jj <Esc>
      noremap H ^
    `;
    applyVimrc(vimrc, mockVim);
    expect(mockVim.noremap).toHaveBeenCalledWith("j", "gj", "normal");
    expect(mockVim.noremap).toHaveBeenCalledWith("k", "gk", "visual");
    expect(mockVim.noremap).toHaveBeenCalledWith("jj", "<Esc>", "insert");
    expect(mockVim.noremap).toHaveBeenCalledWith("H", "^", "normal");
  });

  it("handles targets with spaces", () => {
    const mockVim: VimMock = { noremap: vi.fn(), map: vi.fn() };
    const vimrc = `
      nnoremap <leader>w :w<CR>
      nmap <C-s> :w<CR> :echo "saved"<CR>
    `;
    applyVimrc(vimrc, mockVim);
    expect(mockVim.noremap).toHaveBeenCalledWith("<leader>w", ":w<CR>", "normal");
    expect(mockVim.map).toHaveBeenCalledWith("<C-s>", ':w<CR> :echo "saved"<CR>', "normal");
  });
});
