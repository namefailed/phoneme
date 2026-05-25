import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

export class SectionHook {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    if (Array.isArray(config.hook.commands)) {
      config.hook.command = config.hook.commands.length > 0 ? config.hook.commands[0] : "";
      delete config.hook.commands;
    } else if (config.hook.commands) {
      config.hook.command = config.hook.commands;
      delete config.hook.commands;
    }
    
    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Destination & Integrations</h3>
        <p style="font-size: 12px; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Phoneme can automatically pass your voice notes to other applications or save them to disk by executing a local script. You can point this to a <code>.bat</code> or <code>.ps1</code> file to save notes to Obsidian, Word, or anything else.
        </p>
        <div class="settings-field">
          <label>Integration Script</label>
          <div>
            ${renderField(
              { key: "hook.command", label: "", kind: "text" },
              this.config.hook.command,
            )}
            <button class="inline-button" id="pick-hook">Browse…</button>
            <button class="inline-button" id="test-hook">Test hook</button>
            <div class="test-result" id="hook-result" style="display:none"></div>
            <div class="help">Receives the recording JSON on stdin. The default echoes it to the console.</div>
          </div>
        </div>
        <div class="settings-field">
          <label>Timeout (seconds)</label>
          <div>${renderField(
            { key: "hook.timeout_secs", label: "", kind: "number" },
            this.config.hook.timeout_secs,
          )}</div>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    container.querySelector("#pick-hook")?.addEventListener("click", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({ multiple: false });
      if (typeof path === "string") {
        const input = container.querySelector<HTMLInputElement>(
          `[data-key="hook.command"]`,
        )!;
        // Quote the path if it contains spaces — the daemon splits the
        // command with shlex.
        input.value = path.includes(" ") ? `"${path}"` : path;
        this.config.hook.command = input.value;
      }
    });

    container.querySelector("#test-hook")?.addEventListener("click", async () => {
      const el = container.querySelector<HTMLElement>("#hook-result")!;
      el.style.display = "block";
      el.className = "test-result";
      el.textContent = "Running hook…";
      const input = container.querySelector<HTMLInputElement>(`[data-key="hook.command"]`)!;
      const custom_command = input ? input.value : undefined;
      const result = await invoke<{ ok: boolean; message: string }>(
        "wizard_test_hook",
        { customCommand: custom_command }
      ).catch((e) => ({ ok: false, message: String(e) }));
      el.className = `test-result ${result.ok ? "ok" : "err"}`;
      el.textContent = result.message;
    });
  }
}
