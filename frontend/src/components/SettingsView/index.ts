import { invoke } from "@tauri-apps/api/core";
import { SectionLlm } from "./SectionLlm";
import { SectionRecording } from "./SectionRecording";
import { SectionHotkey } from "./SectionHotkey";
import { SectionHook } from "./SectionHook";
import { SectionStorage } from "./SectionStorage";
import { SectionTray } from "./SectionTray";
import { SectionAdvanced } from "./SectionAdvanced";
import "./styles.css";

export class SettingsView {
  constructor(
    private container: HTMLElement,
    private onClose: () => void,
  ) {
    void this.render();
  }

  private async render() {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const config: any = await invoke("read_config");
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
    new SectionLlm(this.sectionHost(body), config);
    new SectionRecording(this.sectionHost(body), config);
    new SectionHotkey(this.sectionHost(body), config);
    new SectionHook(this.sectionHost(body), config);
    new SectionStorage(this.sectionHost(body), config);
    new SectionTray(this.sectionHost(body), config);
    new SectionAdvanced(this.sectionHost(body), config);

    this.container
      .querySelector("#settings-close")
      ?.addEventListener("click", () => this.onClose());
    this.container
      .querySelector("#settings-save")
      ?.addEventListener("click", async () => {
        try {
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
