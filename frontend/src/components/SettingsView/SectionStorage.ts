import { renderField, bindFieldEvents } from "./form";

export class SectionStorage {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Storage</h3>
        <div class="settings-field">
          <label>Audio directory</label>
          <div>
            ${renderField(
              { key: "recording.audio_dir", label: "", kind: "text" },
              this.config.recording.audio_dir,
            )}
            <button class="inline-button" id="pick-audio-dir">Browse…</button>
            <button class="inline-button" id="open-audio-dir">Open</button>
          </div>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    container
      .querySelector("#pick-audio-dir")
      ?.addEventListener("click", async () => {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const dir = await open({ directory: true, multiple: false });
        if (typeof dir === "string") {
          const input = container.querySelector<HTMLInputElement>(
            `[data-key="recording.audio_dir"]`,
          )!;
          input.value = dir;
          this.config.recording.audio_dir = dir;
        }
      });

    container
      .querySelector("#open-audio-dir")
      ?.addEventListener("click", async () => {
        const { open } = await import("@tauri-apps/plugin-shell");
        await open(this.config.recording.audio_dir).catch(() => {});
      });
  }
}
