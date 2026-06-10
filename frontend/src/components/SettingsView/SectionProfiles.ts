import { errText } from "../../utils/error";
import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { invoke } from "@tauri-apps/api/core";
import {
  listProfiles,
  listProfilesDetailed,
  saveProfile,
  switchProfile,
  deleteProfile,
  renameProfile,
  type ProfileInfo,
} from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { confirmDelete } from "../ConfirmDelete";

const ACTIVE_LS = "phoneme.activeProfile";

/** Human "saved 3h ago" style label from an epoch-ms timestamp. */
function formatSavedAt(ms: number | null): string {
  if (!ms) return "";
  const diff = Date.now() - ms;
  const min = Math.floor(diff / 60000);
  if (min < 1) return "saved just now";
  if (min < 60) return `saved ${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `saved ${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `saved ${day}d ago`;
  return `saved ${new Date(ms).toLocaleDateString()}`;
}

/**
 * Profiles manager (Settings tab). A profile is a full snapshot of
 * `config.toml` under `<config_dir>/profiles/<name>.toml`. Beyond the basic
 * save/switch/delete, this surface adds: per-profile saved-time, an "Active"
 * marker for the last-applied profile, in-place rename, and "Update" (overwrite
 * a profile with the current live config).
 */
@customElement("ph-section-profiles")
export class SectionProfilesElement extends LitElement {
  protected createRenderRoot() {
    return this;
  }

  @state() private profiles: ProfileInfo[] = [];
  @state() private activeName: string | null = null;
  @state() private renamingName: string | null = null;
  @state() private newName = "";
  @state() private busy = false;

  private renameValue = "";

  connectedCallback() {
    super.connectedCallback();
    try {
      this.activeName = localStorage.getItem(ACTIVE_LS);
    } catch {
      /* ignore */
    }
    void this.load();
  }

  private async load() {
    try {
      this.profiles = await listProfilesDetailed();
    } catch {
      // Backend without the detailed command (pre-rebuild): degrade to names.
      try {
        const names = await listProfiles();
        this.profiles = names.map((name) => ({ name, modified_ms: null }));
      } catch (e) {
        showToast(`Failed to load profiles: ${errText(e)}`, "error");
        this.profiles = [];
      }
    }
    // Drop a stale active marker if its profile no longer exists.
    if (this.activeName && !this.profiles.some((p) => p.name === this.activeName)) {
      this.setActive(null);
    }
  }

  private setActive(name: string | null) {
    this.activeName = name;
    try {
      if (name) localStorage.setItem(ACTIVE_LS, name);
      else localStorage.removeItem(ACTIVE_LS);
    } catch {
      /* ignore */
    }
  }

  private async doSwitch(name: string) {
    if (!name || this.busy) return;
    this.busy = true;
    try {
      await switchProfile(name);
      this.setActive(name);
      showToast(`Switched to profile "${name}"`, "success");
      // Switching rewrote config.toml; broadcast the fresh config so the rest
      // of the app (theme, list view) and the open Settings view update and
      // nothing clobbers the new config on the next save.
      const config = await invoke("read_config");
      window.dispatchEvent(new CustomEvent("config:saved", { detail: config }));
    } catch (e) {
      showToast(`Failed to switch profile: ${errText(e)}`, "error");
    } finally {
      this.busy = false;
    }
  }

  private async doUpdate(name: string) {
    if (!name || this.busy) return;
    const confirmed = await confirmDelete({
      title: `Update profile "${name}"?`,
      body: `Overwrite the saved "${name}" profile with your current settings? The settings previously stored in this profile will be replaced.`,
      confirmLabel: "Overwrite",
      skipKey: "phoneme_skip_profile_update_confirm",
    });
    if (!confirmed) return;
    this.busy = true;
    try {
      await saveProfile(name);
      showToast(`Profile "${name}" updated to current settings`, "success");
      await this.load();
    } catch (e) {
      showToast(`Failed to update profile: ${errText(e)}`, "error");
    } finally {
      this.busy = false;
    }
  }

  private startRename(name: string) {
    this.renamingName = name;
    this.renameValue = name;
  }

  private cancelRename() {
    this.renamingName = null;
  }

  private async saveRename(from: string) {
    const to = this.renameValue.trim();
    if (!to) {
      showToast("Profile name cannot be empty", "warning");
      return;
    }
    if (to === from) {
      this.renamingName = null;
      return;
    }
    this.busy = true;
    try {
      await renameProfile(from, to);
      if (this.activeName === from) this.setActive(to);
      showToast(`Renamed "${from}" to "${to}"`, "success");
      this.renamingName = null;
      await this.load();
    } catch (e) {
      showToast(`Failed to rename profile: ${errText(e)}`, "error");
    } finally {
      this.busy = false;
    }
  }

  private async doDelete(name: string) {
    if (!name) return;
    const confirmed = await confirmDelete({
      title: `Delete profile "${name}"?`,
      body: "This permanently deletes the saved profile. Your current settings are not affected. This cannot be undone.",
      confirmLabel: "Delete Profile",
      skipKey: "phoneme_skip_profile_delete_confirm",
    });
    if (!confirmed) return;
    this.busy = true;
    try {
      await deleteProfile(name);
      if (this.activeName === name) this.setActive(null);
      showToast(`Profile "${name}" deleted`, "success");
      await this.load();
    } catch (e) {
      showToast(`Failed to delete profile: ${errText(e)}`, "error");
    } finally {
      this.busy = false;
    }
  }

  private async doSave() {
    const name = this.newName.trim();
    if (!name) {
      showToast("Profile name cannot be empty", "warning");
      return;
    }
    this.busy = true;
    try {
      await saveProfile(name);
      showToast(`Profile "${name}" saved`, "success");
      this.newName = "";
      await this.load();
    } catch (e) {
      showToast(`Failed to save profile: ${errText(e)}`, "error");
    } finally {
      this.busy = false;
    }
  }

  private renderRow(p: ProfileInfo) {
    if (this.renamingName === p.name) {
      return html`
        <div class="tag-mgr-row editing">
          <span class="profile-icon">🗂</span>
          <input type="text" class="tag-mgr-name-input" .value=${this.renameValue}
            @input=${(e: Event) => this.renameValue = (e.target as HTMLInputElement).value}
            @keydown=${(e: KeyboardEvent) => { if (e.key === "Enter") this.saveRename(p.name); if (e.key === "Escape") this.cancelRename(); }} />
          <button class="tag-mgr-save" @click=${() => this.saveRename(p.name)}>Save</button>
          <button class="tag-mgr-cancel" @click=${() => this.cancelRename()}>Cancel</button>
        </div>
      `;
    }

    const active = this.activeName === p.name;
    const saved = formatSavedAt(p.modified_ms);
    return html`
      <div class="tag-mgr-row ${active ? "profile-active" : ""}">
        <span class="profile-icon">🗂</span>
        <span class="profile-id">
          <span class="profile-name">${p.name}</span>
          ${saved ? html`<span class="profile-meta">${saved}</span>` : ""}
        </span>
        ${active ? html`<span class="tag-mgr-badge in-use" title="The profile you last applied">Active</span>` : ""}
        <button class="profile-switch" ?disabled=${this.busy} title="Replace the live config with this profile and reload" @click=${() => this.doSwitch(p.name)}>Switch</button>
        <button class="profile-update" ?disabled=${this.busy} title="Overwrite this profile with your current settings" @click=${() => this.doUpdate(p.name)}>Update</button>
        <button class="profile-rename" ?disabled=${this.busy} title="Rename this profile" @click=${() => this.startRename(p.name)}>Rename</button>
        <button class="profile-delete danger" ?disabled=${this.busy} title="Delete this profile" @click=${() => this.doDelete(p.name)}>Delete</button>
      </div>
    `;
  }

  render() {
    const total = this.profiles.length;
    return html`
      <div class="settings-section">
        <h3>Profiles</h3>
        <p class="settings-help-text" style="margin-bottom: 20px;">
          Save the whole configuration as a named profile (e.g. "work" vs "personal")
          and switch between them here or from the tray. Switching replaces
          <code>config.toml</code> and reloads the daemon. <strong>Update</strong> re-snapshots
          the current settings into a profile; <strong>Rename</strong> renames it in place.
        </p>

        ${total > 0 ? html`
          <div class="tag-mgr-stats">
            <span><span class="tag-mgr-stats-num">${total}</span> profile${total === 1 ? "" : "s"}</span>
            ${this.activeName ? html`
              <span class="tag-mgr-stats-dot">·</span>
              <span>active: <span class="tag-mgr-stats-num">${this.activeName}</span></span>
            ` : ""}
          </div>
        ` : ""}

        <div id="profile-list" class="tag-mgr-list">
          ${total === 0
            ? html`
              <div class="tag-mgr-empty">
                <div class="tag-mgr-empty-icon">🗂</div>
                <p>No saved profiles yet.</p>
                <p class="tag-mgr-empty-hint">Save the current settings as a profile below.</p>
              </div>
            `
            : this.profiles.map((p) => this.renderRow(p))
          }
        </div>

        <div class="tag-mgr-add-section">
          <div class="tag-mgr-add-label">Save current settings as a profile</div>
          <div class="tag-mgr-add-row">
            <input type="text" id="new-profile-name" placeholder="Profile name…" class="tag-mgr-add-name"
              .value=${this.newName}
              @input=${(e: Event) => this.newName = (e.target as HTMLInputElement).value}
              @keydown=${(e: KeyboardEvent) => { if (e.key === "Enter") this.doSave(); }} />
            <button class="primary" id="btn-save-profile" ?disabled=${this.busy} @click=${() => this.doSave()}>Save current as…</button>
          </div>
        </div>
      </div>
    `;
  }
}

export class SectionProfiles {
  private element: SectionProfilesElement;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, _config: any) {
    this.element = document.createElement("ph-section-profiles") as SectionProfilesElement;
    container.appendChild(this.element);
  }
}
