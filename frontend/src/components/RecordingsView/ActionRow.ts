import { deleteRecording, refireHook, replayRecording } from "../../services/ipc";

export type ActionRowCallbacks = {
  onTogglePlay: () => void;
  onRefresh: () => void;
};

export class ActionRow {
  private container: HTMLElement;
  private id: string;
  private cbs: ActionRowCallbacks;

  constructor(container: HTMLElement, id: string, cbs: ActionRowCallbacks) {
    this.container = container;
    this.id = id;
    this.cbs = cbs;
    this.render();
  }

  private render() {
    this.container.innerHTML = `
      <div class="action-row">
        <button class="primary" data-act="play">▶ Play</button>
        <button data-act="replay">↻ Re-transcribe</button>
        <button data-act="refire">⚡ Re-fire hook</button>
        <button data-act="copy">📋 Copy</button>
        <button data-act="reveal">📂 Reveal</button>
        <button class="danger" data-act="delete">🗑 Delete</button>
      </div>
    `;
    this.container.querySelectorAll<HTMLButtonElement>("button[data-act]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const act = btn.dataset.act;
        if (act) void this.handle(act);
      });
    });
  }

  private async handle(act: string) {
    if (act === "play") {
      this.cbs.onTogglePlay();
    } else if (act === "replay") {
      await replayRecording(this.id);
      this.cbs.onRefresh();
    } else if (act === "refire") {
      await refireHook(this.id);
      this.cbs.onRefresh();
    } else if (act === "delete") {
      if (confirm("Delete this recording?")) {
        await deleteRecording(this.id, false);
        this.cbs.onRefresh();
      }
    }
  }
}
