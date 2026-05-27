import { deleteRecording, refireHook, replayRecording } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { invoke } from "@tauri-apps/api/core";

export type ActionRowCallbacks = {
  onTogglePlay: () => void;
  onRefresh: () => void;
  getTranscript: () => string;
  getAudioPath: () => string;
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
        <button class="primary" data-act="play" id="btn-play">▶ Play</button>
        <button data-act="replay">↻ Re-transcribe</button>
        <button data-act="refire">⚡ Re-fire hook</button>
        <button data-act="copy">📋 Copy</button>
        <button data-act="export">⬇ Export</button>
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

  setPlayState(playing: boolean) {
    const btn = this.container.querySelector<HTMLButtonElement>("#btn-play");
    if (btn) {
      btn.textContent = playing ? "⏸ Pause" : "▶ Play";
    }
  }

  private async handle(act: string) {
    if (act === "play") {
      this.cbs.onTogglePlay();
    } else if (act === "replay") {
      try {
        await replayRecording(this.id);
        showToast("Queued for re-transcription", "info");
        this.cbs.onRefresh();
      } catch (e) {
        showToast(`Re-transcribe failed: ${e}`, "error");
      }
    } else if (act === "refire") {
      try {
        await refireHook(this.id);
        showToast("Hook queued", "info");
        this.cbs.onRefresh();
      } catch (e) {
        showToast(`Re-fire hook failed: ${e}`, "error");
      }
    } else if (act === "copy") {
      try {
        await navigator.clipboard.writeText(this.cbs.getTranscript());
        const btn = this.container.querySelector(`button[data-act="copy"]`) as HTMLButtonElement;
        if (btn) {
          const original = btn.innerHTML;
          btn.innerHTML = "✅ Copied!";
          setTimeout(() => { btn.innerHTML = original; }, 2000);
        }
      } catch (e) {
        showToast(`Clipboard copy failed: ${e}`, "error");
      }
    } else if (act === "export") {
      try {
        const { save } = await import("@tauri-apps/plugin-dialog");
        const { writeTextFile } = await import("@tauri-apps/plugin-fs");
        const dest = await save({
          defaultPath: `transcript-${this.id}.txt`,
          filters: [
            { name: "Text", extensions: ["txt"] },
            { name: "All files", extensions: ["*"] },
          ],
        });
        if (dest) {
          await writeTextFile(dest, this.cbs.getTranscript());
          showToast("Transcript exported", "success");
        }
      } catch (e) {
        showToast(`Export failed: ${e}`, "error");
      }
    } else if (act === "reveal") {
      await invoke("reveal_file", { path: this.cbs.getAudioPath() });
    } else if (act === "delete") {
      const { confirmDelete } = await import("../ConfirmDelete");
      if (await confirmDelete()) {
        try {
          await deleteRecording(this.id, false);
          showToast("Recording deleted", "success");
          this.cbs.onRefresh();
        } catch (e) {
          showToast(`Delete failed: ${e}`, "error");
        }
      }
    }
  }
}
