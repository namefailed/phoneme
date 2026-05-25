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
        <div class="settings-field long-input" style="align-items: flex-start;">
          <label style="margin-top: 8px;">Integration Script</label>
          <div style="display: flex; flex-direction: column; gap: 8px; width: 100%;">
            <div style="display: flex; gap: 8px; align-items: center; margin-right: auto;">
              <select id="hook-preset-select" style="background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 4px 8px; font-size: 12px; color: var(--fg-default); max-width: 250px; outline: none; cursor: pointer;">
                <option value="" disabled selected>Load a preset hook...</option>
                <option value="powershell -Command &quot;$d=Get-Content $args[0]|ConvertFrom-Json; Set-Clipboard -Value $d.transcript&quot;">Copy transcript to clipboard</option>
                <option value="powershell -Command &quot;$d=Get-Content $args[0]|ConvertFrom-Json; Add-Content -Path '~/Documents/VoiceNotes.md' -Value &quot;&quot;&quot;$($d.transcript)&quot;&quot;&quot;&quot;">Append to VoiceNotes.md file</option>
                <option value="powershell -Command &quot;$d=Get-Content $args[0]|ConvertFrom-Json; $msg=$d.transcript; Invoke-RestMethod -Uri 'YOUR_WEBHOOK_URL' -Method Post -Body (@{content=$msg}|ConvertTo-Json) -ContentType 'application/json'&quot;">Send to Discord/Slack Webhook</option>
                <option value="python process_note.py">Run custom Python script</option>
              </select>
              <span style="font-size: 11px; color: var(--fg-faded);">← Try these!</span>
            </div>
            <div style="display: flex; gap: 8px; align-items: center; width: 100%;">
              ${renderField(
                { key: "hook.command", label: "", kind: "text" },
                this.config.hook.command,
              )}
              <button class="inline-button" id="pick-hook" style="white-space: nowrap;">Browse…</button>
              <button class="inline-button" id="test-hook" style="white-space: nowrap;">Test hook</button>
            </div>
            <div class="test-result" id="hook-result" style="display:none; margin-top: 0;"></div>
          </div>
          <span style="font-size: 11px; color: var(--fg-faded); display: block;">
            A shell command to run automatically. Phoneme will append the absolute path to a JSON file containing the recording's data to the end of your command. <br/>
            Example: <code>python process.py</code> (will execute as <code>python process.py "C:\path\to\recording.json"</code>).
          </span>
        </div>
        <div class="settings-field">
          <label>Timeout (seconds)</label>
          <div>${renderField(
            { key: "hook.timeout_secs", label: "", kind: "number" },
            this.config.hook.timeout_secs,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            Maximum time (in seconds) to wait for the Integration Script to finish before giving up and labeling the post-processing phase as failed.
          </span>
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

    const presetSelect = container.querySelector<HTMLSelectElement>("#hook-preset-select");
    const cmdInput = container.querySelector<HTMLInputElement>(`[data-key="hook.command"]`);
    if (presetSelect && cmdInput) {
      presetSelect.addEventListener("change", () => {
        if (presetSelect.value) {
          cmdInput.value = presetSelect.value;
          cmdInput.dispatchEvent(new Event("input"));
          this.config.hook.command = presetSelect.value;
        }
      });
    }
  }
}
