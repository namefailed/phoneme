import { errText } from "../../utils/error";
import { LitElement, html, css } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../../utils/toast";

import { SectionWhisper } from "./SectionWhisper";
import { SectionDiarization } from "./SectionDiarization";
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
import "./styles.css";

@customElement('ph-settings-view')
export class SettingsViewElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for global CSS (settings-layout, sv-tab, etc)
  }

  @property({ type: Object }) onClose!: () => void;
  @property({ type: Function }) onNavigateToWizard?: () => void;

  @state() private activeTab: string = "transcription";
  @state() private config: any = null;
  @state() private searchQuery: string = "";
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
      showToast(`Failed to load settings: ${errText(e)}`, "error");
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("config:saved", this.onConfigSaved);
  }

  protected updated(changedProperties: Map<string, any>) {
    if (changedProperties.has('activeTab') || changedProperties.has('config') || changedProperties.has('searchQuery')) {
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
      showToast(`Save failed: ${errText(e)}`, "error");
    }
  }

  private mountSection() {
    if (!this.bodyEl || !this.config) return;
    
    this.bodyEl.innerHTML = "";
    const sectionHost = document.createElement("div");
    this.bodyEl.appendChild(sectionHost);

    const isSearching = this.searchQuery.trim().length > 0;

    const createSubHost = () => {
      const subHost = document.createElement("div");
      sectionHost.appendChild(subHost);
      return subHost;
    };

    const mountAll = () => {
      new SectionWhisper(createSubHost(), this.config);
      new SectionDiarization(createSubHost(), this.config);
      new SectionRecording(createSubHost(), this.config);
      new SectionHotkey(createSubHost(), this.config);
      new SectionInterface(createSubHost(), this.config);
      new SectionEditor(createSubHost(), this.config);
      new SectionTray(createSubHost(), this.config);
      new SectionTags(createSubHost(), this.config);
      new SectionPostProcessing(createSubHost(), this.config);
      new SectionHook(createSubHost(), this.config);
      new SectionStorage(createSubHost(), this.config);
      new SectionProfiles(createSubHost(), this.config);
      new SectionAdvanced(createSubHost(), this.config, this.onNavigateToWizard);
    };

    if (isSearching) {
      mountAll();
      const query = this.searchQuery.toLowerCase();
      const sections = sectionHost.querySelectorAll('.settings-section');
      sections.forEach(sec => {
        let sectionHasMatch = false;
        const fields = sec.querySelectorAll('.settings-field');
        fields.forEach(field => {
          if (field.textContent?.toLowerCase().includes(query)) {
            (field as HTMLElement).style.display = "";
            sectionHasMatch = true;
          } else {
            (field as HTMLElement).style.display = "none";
          }
        });
        const title = sec.querySelector('h3');
        if (title?.textContent?.toLowerCase().includes(query)) {
          sectionHasMatch = true;
          fields.forEach(f => (f as HTMLElement).style.display = "");
        }
        (sec as HTMLElement).style.display = sectionHasMatch ? "" : "none";
      });
    } else {
      switch (this.activeTab) {
        case "transcription":
          new SectionWhisper(createSubHost(), this.config);
          new SectionDiarization(createSubHost(), this.config);
          break;
        case "capture":
          new SectionRecording(createSubHost(), this.config);
          new SectionHotkey(createSubHost(), this.config);
          break;
        case "appearance":
          new SectionInterface(createSubHost(), this.config);
          new SectionEditor(createSubHost(), this.config);
          break;
        case "tags":
          new SectionTags(createSubHost(), this.config);
          break;
        case "postprocessing":
          new SectionPostProcessing(createSubHost(), this.config);
          new SectionHook(createSubHost(), this.config);
          break;
        case "system":
          new SectionStorage(createSubHost(), this.config);
          new SectionProfiles(createSubHost(), this.config);
          new SectionTray(createSubHost(), this.config);
          new SectionAdvanced(createSubHost(), this.config, this.onNavigateToWizard);
          break;
      }
    }
  }

  private switchTab(tab: string) {
    if (this.activeTab !== tab) {
      this.activeTab = tab;
      this.searchQuery = "";
      const searchInput = this.renderRoot.querySelector('.settings-search') as HTMLInputElement;
      if (searchInput) searchInput.value = "";
    }
  }

  private handleSearch(e: Event) {
    this.searchQuery = (e.target as HTMLInputElement).value;
  }

  render() {
    if (!this.config) {
      return html`<div class="error">Loading settings...</div>`;
    }

    const isSearching = this.searchQuery.trim().length > 0;

    return html`
      <div class="settings-layout">
        <div class="settings-sidebar">
          <h2>Settings</h2>
          <input type="search" class="settings-search" placeholder="Search settings..." @input=${this.handleSearch} 
                 style="width: 100%; padding: 8px 12px; margin-bottom: 16px; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 6px; color: var(--fg-default); font-size: 13px;" />
          
          <div class="sv-tab ${this.activeTab === "transcription" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('transcription')}>🗣️ Transcription</div>
          <div class="sv-tab ${this.activeTab === "capture" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('capture')}>🎙️ Capture</div>
          <div class="sv-tab ${this.activeTab === "appearance" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('appearance')}>🎨 Appearance</div>
          <div class="sv-tab ${this.activeTab === "tags" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('tags')}>🏷️ Tags</div>
          <div class="sv-tab ${this.activeTab === "postprocessing" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('postprocessing')}>✨ Post-Processing</div>
          <div class="sv-tab ${this.activeTab === "system" && !isSearching ? "active" : ""}" @click=${() => this.switchTab('system')}>⚙️ System</div>
          
          ${isSearching ? html`<div class="sv-tab active" style="margin-top: 12px; font-style: italic;">Search Results</div>` : ""}
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
  constructor(container: HTMLElement, onClose: () => void, onNavigateToWizard?: () => void) {
    this.element = document.createElement('ph-settings-view') as SettingsViewElement;
    this.element.onClose = onClose;
    this.element.onNavigateToWizard = onNavigateToWizard;
    container.appendChild(this.element);
  }

  public canClose(): boolean {
    return this.element.canClose();
  }

  dispose() {
    this.element.remove();
  }
}
