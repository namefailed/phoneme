import { LitElement, html, css } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../../utils/toast";

import { SectionWhisper } from "./SectionWhisper";
import { SectionRecording } from "./SectionRecording";
import { SectionHotkey } from "./SectionHotkey";
import { SectionHook } from "./SectionHook";
import { SectionStorage } from "./SectionStorage";
import { SectionTray } from "./SectionTray";
import { SectionInterface } from "./SectionInterface";
import { SectionPostProcessing } from "./SectionPostProcessing";
import { SectionEditor } from "./SectionEditor";
import { SectionAdvanced } from "./SectionAdvanced";
import { SectionTags } from "./SectionTags";
import { SectionProfiles } from "./SectionProfiles";

@customElement('ph-settings-view')
export class SettingsViewElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for global CSS (settings-layout, sv-tab, etc)
  }

  @property({ type: Object }) onClose!: () => void;

  @state() private activeTab: string = "whisper";
  @state() private config: any = null;
  private originalConfigStr: string = "";

  @query('#settings-body') bodyEl!: HTMLElement;

  private onConfigSaved = (e: Event) => {
    const detail = (e as CustomEvent).detail;
    if (!detail) return;
    this.config = detail;
    this.originalConfigStr = JSON.stringify(this.config);
    this.mountSection();
  };

  async connectedCallback() {
    super.connectedCallback();
    try {
      this.config = await invoke("read_config");
      this.originalConfigStr = JSON.stringify(this.config);
      window.addEventListener("config:saved", this.onConfigSaved);
    } catch (e) {
      console.error(e);
      showToast(`Failed to load settings: ${e}`, "error");
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("config:saved", this.onConfigSaved);
  }

  protected updated(changedProperties: Map<string, any>) {
    if (changedProperties.has('activeTab') || changedProperties.has('config')) {
      this.mountSection();
    }
  }

  public canClose(): boolean {
    if (this.config && JSON.stringify(this.config) !== this.originalConfigStr) {
      return confirm("You have unsaved changes. Discard them?");
    }
    return true;
  }

  private handleClose() {
    if (this.canClose()) this.onClose();
  }

  private async handleSave() {
    try {
      if (this.config.hook) {
        if (this.config.hook.command !== undefined) {
          if (!Array.isArray(this.config.hook.commands)) {
            this.config.hook.commands = [this.config.hook.command];
          }
          delete this.config.hook.command;
        }
        if (Array.isArray(this.config.hook.commands)) {
          this.config.hook.commands = this.config.hook.commands
            .map((c: unknown) => String(c ?? ""))
            .filter((c: string) => c.trim() !== "");
        }
      }
      await invoke("write_config", { config: this.config });
      window.dispatchEvent(new CustomEvent("config:saved", { detail: this.config }));
      showToast("Settings saved", "success");
      this.onClose();
    } catch (e) {
      showToast(`Save failed: ${e}`, "error");
    }
  }

  private mountSection() {
    if (!this.bodyEl || !this.config) return;
    
    this.bodyEl.innerHTML = "";
    const sectionHost = document.createElement("div");
    this.bodyEl.appendChild(sectionHost);

    switch (this.activeTab) {
      case "whisper": new SectionWhisper(sectionHost, this.config); break;
      case "recording": new SectionRecording(sectionHost, this.config); break;
      case "hotkey": new SectionHotkey(sectionHost, this.config); break;
      case "hook": new SectionHook(sectionHost, this.config); break;
      case "storage": new SectionStorage(sectionHost, this.config); break;
      case "tray": new SectionTray(sectionHost, this.config); break;
      case "interface": new SectionInterface(sectionHost, this.config); break;
      case "post-processing": new SectionPostProcessing(sectionHost, this.config); break;
      case "editor": new SectionEditor(sectionHost, this.config); break;
      case "tags": new SectionTags(sectionHost, this.config); break;
      case "profiles": new SectionProfiles(sectionHost, this.config); break;
      case "advanced": new SectionAdvanced(sectionHost, this.config); break;
    }
  }

  private switchTab(tab: string) {
    if (this.activeTab !== tab) {
      this.activeTab = tab;
    }
  }

  render() {
    if (!this.config) {
      return html`<div class="error">Loading settings...</div>`;
    }

    return html`
      <div class="settings-layout">
        <div class="settings-sidebar">
          <h2>Settings</h2>
          <div class="sv-tab ${this.activeTab === "whisper" ? "active" : ""}" @click=${() => this.switchTab('whisper')}>Whisper</div>
          <div class="sv-tab ${this.activeTab === "recording" ? "active" : ""}" @click=${() => this.switchTab('recording')}>Recording</div>
          <div class="sv-tab ${this.activeTab === "hotkey" ? "active" : ""}" @click=${() => this.switchTab('hotkey')}>Hotkey</div>
          <div class="sv-tab ${this.activeTab === "tray" ? "active" : ""}" @click=${() => this.switchTab('tray')}>System Tray</div>
          <div class="sv-tab ${this.activeTab === "interface" ? "active" : ""}" @click=${() => this.switchTab('interface')}>Interface</div>
          <div class="sv-tab ${this.activeTab === "editor" ? "active" : ""}" @click=${() => this.switchTab('editor')}>Editor</div>
          <div class="sv-tab ${this.activeTab === "post-processing" ? "active" : ""}" @click=${() => this.switchTab('post-processing')}>Post-Processing</div>
          <div class="sv-tab ${this.activeTab === "tags" ? "active" : ""}" @click=${() => this.switchTab('tags')}>Tags</div>
          <div class="sv-tab ${this.activeTab === "profiles" ? "active" : ""}" @click=${() => this.switchTab('profiles')}>Profiles</div>
          <div class="sv-tab ${this.activeTab === "hook" ? "active" : ""}" @click=${() => this.switchTab('hook')}>Action Hook</div>
          <div class="sv-tab ${this.activeTab === "storage" ? "active" : ""}" @click=${() => this.switchTab('storage')}>Storage</div>
          <div class="sv-tab ${this.activeTab === "advanced" ? "active" : ""}" @click=${() => this.switchTab('advanced')}>Advanced</div>
        </div>
        <div class="settings-main" style="display: flex; flex-direction: column; height: 100%;">
          <div class="settings-body" id="settings-body" style="flex: 1; overflow-y: auto;"></div>
          <div class="settings-toolbar" style="padding-top: 16px; border-top: 1px solid var(--border-subtle); display: flex; gap: 8px;">
            <span class="spacer"></span>
            <button id="settings-close" @click=${this.handleClose}>Close</button>
            <button class="primary" id="settings-save" @click=${this.handleSave}>Save</button>
          </div>
        </div>
      </div>
    `;
  }
}

// Legacy wrapper
export class SettingsView {
  private element: SettingsViewElement;
  constructor(container: HTMLElement, onClose: () => void) {
    this.element = document.createElement('ph-settings-view') as SettingsViewElement;
    this.element.onClose = onClose;
    container.appendChild(this.element);
  }

  public canClose(): boolean {
    return this.element.canClose();
  }

  dispose() {
    this.element.remove();
  }
}
