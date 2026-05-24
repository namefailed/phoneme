import { invoke } from "@tauri-apps/api/core";
import { SectionWhisper } from "./SectionWhisper";
import { SectionRecording } from "./SectionRecording";
import { SectionHotkey } from "./SectionHotkey";
import { SectionHook } from "./SectionHook";
import { SectionStorage } from "./SectionStorage";
import { SectionTray } from "./SectionTray";
import { SectionAccessibility } from "./SectionAccessibility";
import { SectionAdvanced } from "./SectionAdvanced";
import "./styles.css";

/**
 * Renders the primary settings window.
 * It fetches the current configuration from the backend, injects sub-sections for each category,
 * and handles saving the mutated config back to disk.
 */
export class SettingsView {
  constructor(
    private container: HTMLElement,
    private onClose: () => void,
  ) {
    void this.render();
  }

  private async render() {
    let config: any;
    try {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      config = await invoke("read_config");
    } catch (e) {
      this.container.innerHTML = `<div class="error">Failed to load settings: ${e}</div>`;
      return;
    }
    
    this.container.innerHTML = `
      <div class="settings-view">
        <div class="settings-toolbar">
          <h2>Settings</h2>
          <span class="spacer"></span>
          <button id="settings-close">Close</button>
          <button class="primary" id="settings-save">Save</button>
        </div>
        <div class="settings-body" id="settings-body"></div>
      </div>
    `;
    const body = this.container.querySelector<HTMLElement>("#settings-body")!;

    // Each section owns its own child div: a Section's render() does
    // `container.innerHTML = …`, so writing them all into `body` directly
    // would have each section clobber the previous one.
    new SectionWhisper(this.sectionHost(body), config);
    new SectionRecording(this.sectionHost(body), config);
    new SectionHotkey(this.sectionHost(body), config);
    new SectionHook(this.sectionHost(body), config);
    new SectionStorage(this.sectionHost(body), config);
    new SectionTray(this.sectionHost(body), config);
    new SectionAccessibility(this.sectionHost(body), config);
    new SectionAdvanced(this.sectionHost(body), config);

    this.container
      .querySelector("#settings-close")
      ?.addEventListener("click", () => this.onClose());
    this.container
      .querySelector("#settings-save")
      ?.addEventListener("click", async () => {
        try {
          // The backend serialization uses `commands`
          if (config.hook && config.hook.command !== undefined) {
            config.hook.commands = config.hook.command;
            delete config.hook.command;
          }
          await invoke("write_config", { config });
          this.onClose();
        } catch (e) {
          alert(`Save failed: ${e}`);
        }
      });
  }

  private sectionHost(body: HTMLElement): HTMLElement {
    const el = document.createElement("div");
    body.appendChild(el);
    return el;
  }

  dispose() {}
}
