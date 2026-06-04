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

  private async render() {
    this.container.innerHTML = `
      <div class="action-row">
        <button class="primary" data-act="play" id="btn-play">▶ Play</button>
        <div style="display: flex; align-items: center; border: 1px solid color-mix(in srgb, var(--accent) 50%, transparent); border-radius: 6px; background: var(--bg-deep);">
          <button data-act="replay" style="border: none; background: transparent; padding-right: 4px; box-shadow: none;">↻ Re-transcribe</button>
          <select id="replay-model" style="border: none; background: transparent; padding-left: 4px; padding-right: 20px; font-size: 11px; max-width: 90px; box-shadow: none;">
            <option value="">Default</option>
          </select>
        </div>
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

    try {
      const downloaded = await invoke<string[]>("wizard_list_downloaded_models");
      const sel = this.container.querySelector<HTMLSelectElement>("#replay-model");
      if (sel) {
        const filenames = ["ggml-tiny.en.bin", "ggml-base.en.bin", "ggml-small.en.bin", "ggml-medium.en.bin", "ggml-large-v3.bin"];
        for (const f of filenames) {
          const path = downloaded.find((p: string) => p.endsWith(f));
          if (path) {
            const name = f.replace("ggml-", "").replace(".en.bin", "").replace(".bin", "");
            const opt = document.createElement("option");
            opt.value = path;
            opt.textContent = name;
            sel.appendChild(opt);
          }
        }
      }
    } catch (e) {
      console.error(e);
    }
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
        const sel = this.container.querySelector<HTMLSelectElement>("#replay-model");
        const override = sel?.value || undefined;
        await replayRecording(this.id, override);
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
      try {
        await invoke("reveal_file", { path: this.cbs.getAudioPath() });
      } catch (e) {
        showToast(`Reveal failed: ${e}`, "error");
      }
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
