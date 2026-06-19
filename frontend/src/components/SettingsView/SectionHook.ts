import { renderField, bindFieldEvents } from "./form";
import { escapeAttr } from "../../utils/format";

/**
 * Settings → Integrations, OUTBOUND half. Post-transcription scripts and webhook
 * URLs now live in the Playbook (Hook entries on a recipe); what stays here is
 * the GLOBAL `[webhook]` network/safety policy that governs every outbound POST a
 * Playbook webhook hook makes — the SSRF guard, HMAC signing, and custom headers.
 * A pointer card sends users to the Playbook to add the actual hooks.
 * Plain section class on the form.ts binding.
 */
export class SectionHook {
  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    // Seed the [webhook] table so the policy fields can bind to it (setByPath
    // throws on a missing parent). The knobs default off — the safe posture the
    // backend ships. `hmac_secret` arrives masked (the tray replaces a set secret
    // with the keep-sentinel before it reaches us).
    const w = config.webhook ?? (config.webhook = {});
    if (typeof w.allow_private_network !== "boolean") w.allow_private_network = false;
    if (typeof w.allow_http !== "boolean") w.allow_http = false;
    if (typeof w.hmac_secret !== "string") w.hmac_secret = "";
    if (typeof w.custom_headers !== "object" || w.custom_headers === null || Array.isArray(w.custom_headers)) {
      w.custom_headers = {};
    }
    // Edit headers via an ordered row list, synced back to the map on every change
    // (a plain object can't be edited row-by-row while keys are renamed).
    this.headerRows = Object.entries(w.custom_headers as Record<string, string>).map(
      ([name, value]) => ({ name, value: String(value ?? "") }),
    );

    this.render(container);
  }

  /** Working copy of `webhook.custom_headers`, synced to the config map on edit. */
  private headerRows: { name: string; value: string }[] = [];

  /** Rebuild `config.webhook.custom_headers` from the row list, dropping rows
   *  with a blank name and trimming names (last write wins on a duplicate). */
  private syncHeaders() {
    const map: Record<string, string> = {};
    for (const r of this.headerRows) {
      const name = r.name.trim();
      if (name) map[name] = r.value;
    }
    this.config.webhook.custom_headers = map;
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Outbound (webhook policy)</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Post-transcription <b>scripts and webhooks</b> are now <b>Hook entries</b> in the
          <b>Playbook</b> — add one, set its command or webhook URL and trigger, then drop it
          into a recipe. The settings below are the <b>global policy</b> that governs every
          outbound webhook those hooks make: the SSRF guard, HMAC signing, and headers.
        </p>
        <div class="settings-field">
          <label>Add a hook</label>
          <div>
            <button class="inline-button" id="hook-goto-playbook" type="button">Open the Playbook →</button>
          </div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            Shell commands, keyword-triggered actions, and outbound webhooks all live as Hook
            entries there. Existing <code>[hook]</code> config was migrated once on upgrade.
          </span>
        </div>
        <div class="settings-field">
          <label>Allow private network</label>
          <div>${renderField(
            { key: "webhook.allow_private_network", label: "", kind: "checkbox" },
            this.config.webhook.allow_private_network ?? false,
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            Allow webhook hooks to POST to private network addresses (your LAN, <code>10.x</code> / <code>192.168.x</code> / <code>172.16–31.x</code>, link-local). Off by default — such targets are blocked to stop a transcript being sent to an internal service by mistake. <b>Only enable for local automation you trust</b> (e.g. an n8n box on your NAS).
          </span>
        </div>
        <div class="settings-field">
          <label>Allow insecure HTTP</label>
          <div>${renderField(
            { key: "webhook.allow_http", label: "", kind: "checkbox" },
            this.config.webhook.allow_http ?? false,
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            Allow plain <code>http://</code> for public webhook targets. Off by default — public URLs must be <code>https://</code> so transcripts aren't sent in the clear. Loopback is always allowed; <b>leave this off unless you really mean to send over unencrypted HTTP</b>.
          </span>
        </div>
        <div class="settings-field">
          <label>Webhook signing secret</label>
          <div>${renderField(
            { key: "webhook.hmac_secret", label: "", kind: "text", type: "password", placeholder: "Optional — signs each webhook POST" },
            this.config.webhook.hmac_secret ?? "",
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            When set, every webhook POST carries an <code>X-Phoneme-Signature: sha256=&lt;hex&gt;</code> header — HMAC-SHA256 of the exact body — so your receiver can verify the request really came from this Phoneme install and wasn't tampered with. Leave blank to disable signing. Stored encrypted (DPAPI); it never leaves your machine in plaintext.
          </span>
        </div>
        <div class="settings-field stacked">
          <label>Webhook headers</label>
          <div id="wh-headers-list" style="display: flex; flex-direction: column; gap: 8px; align-items: stretch;"></div>
          <button class="inline-button" id="wh-add-header" style="margin-top: 8px; align-self: flex-start;">+ Add header</button>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 6px; display: block;">
            Extra HTTP headers attached to every webhook POST (e.g. <code>Authorization: Bearer …</code>, an <code>X-Api-Key</code>, or a routing tag). A header that collides with one Phoneme sets itself (<code>Content-Type</code>, the signature header) is ignored — Phoneme's value wins.
          </span>
        </div>
        <div class="settings-field">
          <label>Logs</label>
          <div>
            <button class="inline-button" id="hook-goto-logs" type="button">View logs in System →</button>
          </div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            Debugging a hook that silently does nothing? The <code>hook.log</code> and <code>daemon.log</code> viewers live under <b>System → Diagnostics</b>, alongside the daemon log level.
          </span>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    container.querySelector("#hook-goto-playbook")?.addEventListener("click", () => {
      window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view: "settings", section: "managers/playbook" } }));
    });
    container.querySelector("#hook-goto-logs")?.addEventListener("click", () => {
      window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view: "settings", section: "system" } }));
    });

    this.renderHeaders(container);
    container.querySelector("#wh-add-header")?.addEventListener("click", () => {
      this.headerRows.push({ name: "", value: "" });
      this.renderHeaders(container);
    });
  }

  /** Render the webhook custom-header rows and wire their inputs. */
  private renderHeaders(container: HTMLElement) {
    const list = container.querySelector<HTMLElement>("#wh-headers-list");
    if (!list) return;
    if (this.headerRows.length === 0) {
      list.innerHTML = `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No custom headers.</span>`;
      return;
    }
    list.innerHTML = this.headerRows
      .map(
        (h, i) => `
        <div class="wh-header-row" data-idx="${i}" style="display: flex; gap: 6px; align-items: center;">
          <input class="wh-name" type="text" placeholder="Header name (e.g. Authorization)" value="${escapeAttr(h.name)}"
            style="width: 220px; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 5px 8px; font-size: 0.8571rem; color: var(--fg-default);" />
          <input class="wh-value" type="text" placeholder="Value" value="${escapeAttr(h.value)}"
            style="flex: 1; min-width: 0; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 5px 8px; font-size: 0.8571rem; color: var(--fg-default);" />
          <button class="inline-button wh-remove" title="Remove header" style="padding: 4px 8px;">✕</button>
        </div>`,
      )
      .join("");

    list.querySelectorAll<HTMLElement>(".wh-header-row").forEach((row) => {
      const idx = Number(row.dataset.idx);
      row.querySelector<HTMLInputElement>(".wh-name")?.addEventListener("input", (e) => {
        this.headerRows[idx].name = (e.target as HTMLInputElement).value;
        this.syncHeaders();
      });
      row.querySelector<HTMLInputElement>(".wh-value")?.addEventListener("input", (e) => {
        this.headerRows[idx].value = (e.target as HTMLInputElement).value;
        this.syncHeaders();
      });
      row.querySelector(".wh-remove")?.addEventListener("click", () => {
        this.headerRows.splice(idx, 1);
        this.syncHeaders();
        this.renderHeaders(container);
      });
    });
  }
}
