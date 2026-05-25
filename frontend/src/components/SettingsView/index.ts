import { invoke } from "@tauri-apps/api/core";
import { SectionWhisper } from "./SectionWhisper";
import { SectionRecording } from "./SectionRecording";
import { SectionHotkey } from "./SectionHotkey";
import { SectionHook } from "./SectionHook";
import { SectionStorage } from "./SectionStorage";
import { SectionTray } from "./SectionTray";
import { SectionInterface } from "./SectionInterface";
import { SectionAccessibility } from "./SectionAccessibility";
import { SectionEditor } from "./SectionEditor";
import { SectionAdvanced } from "./SectionAdvanced";
import "./styles.css";

/**
 * Renders the primary settings window.
 * It fetches the current configuration from the backend, injects sub-sections for each category,
 * and handles saving the mutated config back to disk.
 */
export class SettingsView {
  private activeTab: string = "whisper";
  private config: any = null;
  private originalConfigStr: string = "";

  constructor(
    private container: HTMLElement,
    private onClose: () => void,
  ) {
    void this.init();
  }

  private async init() {
    try {
      this.config = await invoke("read_config");
      this.originalConfigStr = JSON.stringify(this.config);
      this.render();
    } catch (e) {
      this.container.innerHTML = `<div class="error">Failed to load settings: ${e}</div>`;
      return;
    }
  }

  public canClose(): boolean {
    if (this.config && JSON.stringify(this.config) !== this.originalConfigStr) {
      return confirm("You have unsaved changes. Discard them?");
    }
    return true;
  }

  private render() {
    const config = this.config;
    if (!config) return;
    
    this.container.innerHTML = `
      <div class="settings-layout">
        <div class="settings-sidebar">
          <h2>Settings</h2>
          <div class="sv-tab ${this.activeTab === "whisper" ? "active" : ""}" data-tab="whisper">Whisper</div>
          <div class="sv-tab ${this.activeTab === "recording" ? "active" : ""}" data-tab="recording">Recording</div>
          <div class="sv-tab ${this.activeTab === "hotkey" ? "active" : ""}" data-tab="hotkey">Hotkey</div>
          <div class="sv-tab ${this.activeTab === "tray" ? "active" : ""}" data-tab="tray">System Tray</div>
          <div class="sv-tab ${this.activeTab === "interface" ? "active" : ""}" data-tab="interface">Interface</div>
          <div class="sv-tab ${this.activeTab === "editor" ? "active" : ""}" data-tab="editor">Editor</div>
          <div class="sv-tab ${this.activeTab === "accessibility" ? "active" : ""}" data-tab="accessibility">Post-Processing</div>
          <div class="sv-tab ${this.activeTab === "hook" ? "active" : ""}" data-tab="hook">Action Hook</div>
          <div class="sv-tab ${this.activeTab === "storage" ? "active" : ""}" data-tab="storage">Storage</div>
          <div class="sv-tab ${this.activeTab === "advanced" ? "active" : ""}" data-tab="advanced">Advanced</div>
        </div>
        <div class="settings-main">
          <div class="settings-toolbar">
            <span class="spacer"></span>
            <button id="settings-close">Close</button>
            <button class="primary" id="settings-save">Save</button>
          </div>
          <div class="settings-body" id="settings-body"></div>
        </div>
      </div>
    `;
    const body = this.container.querySelector<HTMLElement>("#settings-body")!;
    
    // Each section owns its own child div: a Section's render() does
    // `container.innerHTML = …`, so writing them all into `body` directly
    // would have each section clobber the previous one.
    switch (this.activeTab) {
      case "whisper": new SectionWhisper(this.sectionHost(body), config); break;
      case "recording": new SectionRecording(this.sectionHost(body), config); break;
      case "hotkey": new SectionHotkey(this.sectionHost(body), config); break;
      case "hook": new SectionHook(this.sectionHost(body), config); break;
      case "storage": new SectionStorage(this.sectionHost(body), config); break;
      case "tray": new SectionTray(this.sectionHost(body), config); break;
      case "interface": new SectionInterface(this.sectionHost(body), config); break;
      case "accessibility": new SectionAccessibility(this.sectionHost(body), config); break;
      case "editor": new SectionEditor(this.sectionHost(body), config); break;
      case "advanced": new SectionAdvanced(this.sectionHost(body), config); break;
    }

    this.container.querySelectorAll<HTMLElement>(".sv-tab").forEach(tab => {
      tab.addEventListener("click", () => {
        const target = tab.dataset.tab;
        if (target && target !== this.activeTab) {
          this.activeTab = target;
          this.render(); // Re-render the UI with the new active tab
        }
      });
    });

    this.container
      .querySelector("#settings-close")
      ?.addEventListener("click", () => {
        if (this.canClose()) this.onClose();
      });
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
          window.dispatchEvent(new CustomEvent("config:saved", { detail: config }));
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
