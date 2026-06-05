import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";
import { escapeAttr } from "../../utils/format";

type KeywordRule = { pattern: string; command: string; case_sensitive: boolean };

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
    if (!Array.isArray(config.hook.keyword_rules)) {
      config.hook.keyword_rules = [];
    }

    this.render(container);
  }

  private get rules(): KeywordRule[] {
    return this.config.hook.keyword_rules as KeywordRule[];
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
                <optgroup label="Clipboard">
                  <option value="powershell -Command &quot;$d=($input|Out-String|ConvertFrom-Json); Set-Clipboard -Value $d.transcript&quot;">Copy transcript to clipboard</option>
                </optgroup>
                <optgroup label="Files &amp; Notes">
                  <option value="powershell -Command &quot;$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\VoiceNotes.md') -Value $d.transcript&quot;">Append to VoiceNotes.md</option>
                  <option value="powershell -Command &quot;$d=($input|Out-String|ConvertFrom-Json); $ts=(Get-Date -Format 'yyyy-MM-dd HH:mm'); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\phoneme.log') -Value &quot;&quot;&quot;[$ts] $($d.transcript)&quot;&quot;&quot;&quot;">Append to timestamped log</option>
                  <option value="powershell -Command &quot;$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\todo.txt') -Value &quot;&quot;&quot;[ ] $($d.transcript)&quot;&quot;&quot;&quot;">Add to todo.txt</option>
                  <option value="powershell -Command &quot;$d=($input|Out-String|ConvertFrom-Json); $date=(Get-Date -Format 'yyyy-MM-dd'); $obsidian=Join-Path $env:USERPROFILE 'Documents\Obsidian\Daily\'+$date+'.md'; Add-Content -Path $obsidian -Value &quot;&quot;&quot;\`n## Voice Note\`n$($d.transcript)&quot;&quot;&quot;&quot;">Append to Obsidian daily note</option>
                </optgroup>
                <optgroup label="Web &amp; Webhooks">
                  <option value="powershell -Command &quot;$d=($input|Out-String|ConvertFrom-Json); Invoke-RestMethod -Uri 'YOUR_DISCORD_WEBHOOK_URL' -Method Post -Body (@{content=$d.transcript}|ConvertTo-Json) -ContentType 'application/json'&quot;">Discord webhook</option>
                  <option value="powershell -Command &quot;$d=($input|Out-String|ConvertFrom-Json); Invoke-RestMethod -Uri 'YOUR_SLACK_WEBHOOK_URL' -Method Post -Body (@{text=$d.transcript}|ConvertTo-Json) -ContentType 'application/json'&quot;">Slack webhook</option>
                </optgroup>
                <optgroup label="Scripts">
                  <option value="python process_note.py">Run Python script</option>
                  <option value="node process_note.js">Run Node.js script</option>
                </optgroup>
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
            A shell command to run automatically. Phoneme will pipe a JSON object containing the recording's data to your command's standard input (<code>stdin</code>). <br/>
            Example: <code>python process.py</code> (will execute as <code>python process.py &lt; data.json</code>).
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
        <div class="settings-field">
          <label>Run hooks after transcription</label>
          <div>${renderField(
            { key: "hook.run_on_transcribe", label: "", kind: "checkbox" },
            this.config.hook.run_on_transcribe ?? true,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            When on (default), your Integration Script and webhook fire automatically after every transcription — including re-transcriptions. Turn it off if you only want hooks to run on demand via the <b>⚡ Re-fire hook</b> button (so re-transcribing fixes the text without re-triggering side effects like re-appending to a note).
          </span>
        </div>
        <div class="settings-field stacked">
          <label>Keyword-triggered hooks</label>
          <div id="kw-rules-list" style="display: flex; flex-direction: column; gap: 8px;"></div>
          <button class="inline-button" id="kw-add-rule" style="margin-top: 8px; align-self: flex-start;">+ Add rule</button>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 6px; display: block;">
            Run an extra command <i>only</i> when the transcript contains a phrase — on top of the Integration Script above. Example: phrase <code>Action Item:</code> → a command that sends the note to your task manager. The command receives the same JSON on <code>stdin</code>.
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

    this.renderKwRules(container);
    container.querySelector("#kw-add-rule")?.addEventListener("click", () => {
      this.rules.push({ pattern: "", command: "", case_sensitive: false });
      this.renderKwRules(container);
    });
  }

  /** Render the keyword-rule rows from config and wire their inputs. */
  private renderKwRules(container: HTMLElement) {
    const list = container.querySelector<HTMLElement>("#kw-rules-list");
    if (!list) return;
    if (this.rules.length === 0) {
      list.innerHTML = `<span style="font-size: 11px; color: var(--fg-faded);">No keyword rules yet.</span>`;
      return;
    }
    list.innerHTML = this.rules
      .map(
        (r, i) => `
        <div class="kw-rule-row" data-idx="${i}" style="display: flex; gap: 6px; align-items: center;">
          <input class="kw-pattern" type="text" placeholder="Phrase to match…" value="${escapeAttr(r.pattern)}"
            style="width: 180px; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 5px 8px; font-size: 12px; color: var(--fg-default);" />
          <input class="kw-command" type="text" placeholder="Command to run…" value="${escapeAttr(r.command)}"
            style="flex: 1; min-width: 0; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 5px 8px; font-size: 12px; color: var(--fg-default);" />
          <label title="Case-sensitive match" style="display: flex; align-items: center; gap: 4px; font-size: 11px; color: var(--fg-muted); white-space: nowrap;">
            <input type="checkbox" class="kw-cs" ${r.case_sensitive ? "checked" : ""} /> Aa
          </label>
          <button class="inline-button kw-remove" title="Remove rule" style="padding: 4px 8px;">✕</button>
        </div>`,
      )
      .join("");

    list.querySelectorAll<HTMLElement>(".kw-rule-row").forEach((row) => {
      const idx = Number(row.dataset.idx);
      row.querySelector<HTMLInputElement>(".kw-pattern")?.addEventListener("input", (e) => {
        this.rules[idx].pattern = (e.target as HTMLInputElement).value;
      });
      row.querySelector<HTMLInputElement>(".kw-command")?.addEventListener("input", (e) => {
        this.rules[idx].command = (e.target as HTMLInputElement).value;
      });
      row.querySelector<HTMLInputElement>(".kw-cs")?.addEventListener("change", (e) => {
        this.rules[idx].case_sensitive = (e.target as HTMLInputElement).checked;
      });
      row.querySelector(".kw-remove")?.addEventListener("click", () => {
        this.rules.splice(idx, 1);
        this.renderKwRules(container);
      });
    });
  }
}
