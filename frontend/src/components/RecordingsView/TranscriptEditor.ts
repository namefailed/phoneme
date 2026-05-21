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
      <textarea class="transcript-textarea" rows="6">${escape(this.initial)}</textarea>
    `;
    const ta = this.container.querySelector<HTMLTextAreaElement>(".transcript-textarea");
    if (!ta) return;
    autosize(ta);
    ta.addEventListener("input", () => {
      this.current = ta.value;
      this.onDirtyChange(this.current !== this.initial);
    });
    ta.addEventListener("keydown", (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s") {
        e.preventDefault();
        void this.save();
      }
    });
  }

  async save() {
    if (this.current === this.initial) return;
    await updateTranscript(this.id, this.current);
    this.initial = this.current;
    this.onDirtyChange(false);
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
