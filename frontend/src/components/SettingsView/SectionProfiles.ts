import { invoke } from "@tauri-apps/api/core";
import {
  listProfiles,
  saveProfile,
  switchProfile,
  deleteProfile,
} from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { escapeHtml, escapeAttr } from "../../utils/format";
import { confirmDelete } from "../ConfirmDelete";

/**
 * Profiles manager — lives in the Settings sidebar as its own tab.
 *
 * A profile is a full snapshot of `config.toml` stored under
 * `<config_dir>/profiles/<name>.toml`. "Save current as…" snapshots the live
 * config; "Switch" copies a profile over `config.toml` and reloads the daemon.
 * Because switching replaces the whole config, the Settings view is reloaded
 * afterward via the `config:saved` event so the open form reflects the change.
 */
export class SectionProfiles {
  private container: HTMLElement;
  private profiles: string[] = [];

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, _config: any) {
    this.container = container;
    void this.load();
  }

  private async load() {
    try {
      this.profiles = await listProfiles();
    } catch (e) {
      showToast(`Failed to load profiles: ${e}`, "error");
      this.profiles = [];
    }
    this.render();
  }

  private render() {
    const rows = this.profiles
      .map(
        (name) => `
        <div class="tag-mgr-row" data-profile="${escapeAttr(name)}">
          <span class="tag-mgr-name">${escapeHtml(name)}</span>
          <button class="profile-switch" data-profile="${escapeAttr(name)}">Switch</button>
          <button class="profile-delete danger" data-profile="${escapeAttr(name)}">Delete</button>
        </div>`,
      )
      .join("");

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Profiles</h3>
        <p class="settings-help-text" style="margin-bottom: 20px;">
          Save the whole configuration as a named profile (e.g. "work" vs "personal")
          and switch between them here or from the tray. Switching replaces
          <code>config.toml</code> and reloads the daemon.
        </p>

        <div id="profile-list" style="display: flex; flex-direction: column; gap: 6px; margin-bottom: 24px;">
          ${
            this.profiles.length === 0
              ? `<div class="tag-mgr-empty">
                   <div class="tag-mgr-empty-icon">🗂</div>
                   <p>No saved profiles yet.</p>
                   <p class="tag-mgr-empty-hint">Save the current settings as a profile below.</p>
                 </div>`
              : rows
          }
        </div>

        <div class="tag-mgr-add-section">
          <div class="tag-mgr-add-label">Save current settings as a profile</div>
          <div class="tag-mgr-add-row">
            <input
              type="text"
              id="new-profile-name"
              placeholder="Profile name…"
              class="tag-mgr-add-name"
            />
            <button class="primary" id="btn-save-profile">Save current as…</button>
          </div>
        </div>
      </div>
    `;

    this.bindEvents();
  }

  private bindEvents() {
    // ── Switch ────────────────────────────────────────────────────────────────
    this.container.querySelectorAll<HTMLButtonElement>(".profile-switch").forEach((btn) => {
      btn.addEventListener("click", () => void this.doSwitch(btn.dataset.profile ?? ""));
    });

    // ── Delete ────────────────────────────────────────────────────────────────
    this.container.querySelectorAll<HTMLButtonElement>(".profile-delete").forEach((btn) => {
      btn.addEventListener("click", () => void this.doDelete(btn.dataset.profile ?? ""));
    });

    // ── Save current as… ───────────────────────────────────────────────────────
    const saveBtn = this.container.querySelector<HTMLButtonElement>("#btn-save-profile");
    const nameInput = this.container.querySelector<HTMLInputElement>("#new-profile-name");

    const doSave = async () => {
      const name = nameInput?.value.trim() ?? "";
      if (!name) {
        showToast("Profile name cannot be empty", "warning");
        nameInput?.focus();
        return;
      }
      try {
        await saveProfile(name);
        showToast(`Profile "${name}" saved`, "success");
        if (nameInput) nameInput.value = "";
        await this.load();
      } catch (e) {
        showToast(`Failed to save profile: ${e}`, "error");
      }
    };

    saveBtn?.addEventListener("click", () => void doSave());
    nameInput?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void doSave();
    });
  }

  private async doSwitch(name: string) {
    if (!name) return;
    try {
      await switchProfile(name);
      showToast(`Switched to profile "${name}"`, "success");
      // Switching rewrote config.toml; broadcast the fresh config so the rest
      // of the app (theme, list view) and the open Settings view update and
      // nothing clobbers the new config on the next save.
      const config = await invoke("read_config");
      window.dispatchEvent(new CustomEvent("config:saved", { detail: config }));
    } catch (e) {
      showToast(`Failed to switch profile: ${e}`, "error");
    }
  }

  private async doDelete(name: string) {
    if (!name) return;
    const confirmed = await confirmDelete({
      title: `Delete profile "${escapeHtml(name)}"?`,
      body: "This permanently deletes the saved profile. Your current settings are not affected. This cannot be undone.",
      confirmLabel: "Delete Profile",
      skipKey: "phoneme_skip_profile_delete_confirm",
    });
    if (!confirmed) return;
    try {
      await deleteProfile(name);
      showToast(`Profile "${name}" deleted`, "success");
      await this.load();
    } catch (e) {
      showToast(`Failed to delete profile: ${e}`, "error");
    }
  }
}
