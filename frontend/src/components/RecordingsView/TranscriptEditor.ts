import { updateTranscript } from "../../services/ipc";

export class TranscriptEditor {
  private container: HTMLElement;
  private id: string;
  private initial: string;
  private current: string;
  private onDirtyChange: (dirty: boolean) => void;

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
    this.render();
  }

  private render() {
    this.container.innerHTML = `
      <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px;">
        <span style="font-size: 11px; font-weight: bold; text-transform: uppercase; color: var(--fg-muted);">Transcript</span>
        <button id="btn-save-transcript" style="display: none; background: var(--accent); color: var(--accent-fg); border: none; padding: 4px 10px; border-radius: 4px; font-size: 11px; cursor: pointer;">Save Changes</button>
      </div>
      <textarea class="transcript-textarea" rows="6">${escape(this.initial)}</textarea>
    `;
    const ta = this.container.querySelector<HTMLTextAreaElement>(".transcript-textarea");
    const saveBtn = this.container.querySelector<HTMLButtonElement>("#btn-save-transcript");
    
    if (!ta) return;
    autosize(ta);
    
    const updateSaveBtn = () => {
      if (saveBtn) {
        saveBtn.style.display = this.current !== this.initial ? "block" : "none";
      }
    };

    ta.addEventListener("input", () => {
      this.current = ta.value;
      this.onDirtyChange(this.current !== this.initial);
      updateSaveBtn();
    });
    
    ta.addEventListener("keydown", (e) => {
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
}

function autosize(ta: HTMLTextAreaElement) {
  const resize = () => {
    ta.style.height = "auto";
    ta.style.height = ta.scrollHeight + "px";
  };
  ta.addEventListener("input", resize);
  resize();
}

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
