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
    // Normalize the hook commands to a string[] (the backend serializes
    // `commands: Vec<String>`; older configs / the previous UI used a single
    // `command`). Editing the array directly means we no longer silently drop
    // additional commands the way the old single-field UI did.
    const h = config.hook ?? (config.hook = {});
    let cmds: string[];
    if (Array.isArray(h.commands)) {
      cmds = h.commands.map((c: unknown) => String(c ?? ""));
    } else if (typeof h.commands === "string" && h.commands) {
      cmds = [h.commands];
    } else if (typeof h.command === "string" && h.command) {
      cmds = [h.command];
    } else {
      cmds = [];
    }
    h.commands = cmds;
    delete h.command; // legacy field — we manage `commands` from here on
    if (!Array.isArray(h.keyword_rules)) h.keyword_rules = [];

    this.render(container);
  }

  private get commands(): string[] {
    return this.config.hook.commands as string[];
  }

  private get rules(): KeywordRule[] {
    return this.config.hook.keyword_rules as KeywordRule[];
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Destination & Integrations</h3>
        <p style="font-size: 12px; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Phoneme can automatically pass your voice notes to other applications or save them to disk by executing local scripts. Point these at a <code>.bat</code> or <code>.ps1</code> file to save notes to Obsidian, Word, or anything else. Multiple commands run sequentially in order.
        </p>
        <div class="settings-field long-input" style="align-items: flex-start;">
          <label style="margin-top: 8px;">Integration Scripts</label>
          <div style="display: flex; flex-direction: column; gap: 8px; width: 100%; align-items: stretch;">
            <div style="display: flex; gap: 8px; align-items: center; margin-right: auto;">
              <select id="hook-preset-select" style="background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 4px 8px; font-size: 12px; color: var(--fg-default); max-width: 250px; outline: none; cursor: pointer;">
                <option value="" disabled selected>Add a preset hook…</option>
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
              <span style="font-size: 11px; color: var(--fg-faded);">← adds a command below</span>
            </div>
            <div id="hook-cmd-list" style="display: flex; flex-direction: column; gap: 8px; align-items: stretch;"></div>
            <button class="inline-button" id="hook-add-cmd" style="align-self: flex-start;">+ Add command</button>
            <div class="test-result" id="hook-result" style="display:none; margin-top: 0;"></div>
          </div>
          <span style="font-size: 11px; color: var(--fg-faded); display: block;">
            Each command runs automatically (in order) after transcription. Phoneme pipes a JSON object with the recording's data to the command's standard input (<code>stdin</code>). <br/>
            Example: <code>python process.py</code> (runs as <code>python pipes.py &lt; data.json</code>).
          </span>
        </div>
        <div class="settings-field">
          <label>Timeout (seconds)</label>
          <div>${renderField(
            { key: "hook.timeout_secs", label: "", kind: "number" },
            this.config.hook.timeout_secs,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            Maximum time (in seconds) to wait for each Integration Script to finish before giving up and labeling the post-processing phase as failed.
          </span>
        </div>
        <div class="settings-field">
          <label>Run hooks after transcription</label>
          <div>${renderField(
            { key: "hook.run_on_transcribe", label: "", kind: "checkbox" },
            this.config.hook.run_on_transcribe ?? true,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            When on (default), your Integration Scripts and webhook fire automatically after every transcription — including re-transcriptions. Turn it off if you only want hooks to run on demand via the <b>⚡ Re-fire hook</b> button (so re-transcribing fixes the text without re-triggering side effects like re-appending to a note).
          </span>
        </div>
        <div class="settings-field stacked">
          <label>Keyword-triggered hooks</label>
          <div id="kw-rules-list" style="display: flex; flex-direction: column; gap: 8px; align-items: stretch;"></div>
          <button class="inline-button" id="kw-add-rule" style="margin-top: 8px; align-self: flex-start;">+ Add rule</button>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 6px; display: block;">
            Run an extra command <i>only</i> when the transcript contains a phrase — on top of the Integration Scripts above. Example: phrase <code>Action Item:</code> → a command that sends the note to your task manager. The command receives the same JSON on <code>stdin</code>.
          </span>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    this.renderCmds(container);
    container.querySelector("#hook-add-cmd")?.addEventListener("click", () => {
      this.commands.push("");
      this.renderCmds(container);
    });

    const presetSelect = container.querySelector<HTMLSelectElement>("#hook-preset-select");
    presetSelect?.addEventListener("change", () => {
      if (presetSelect.value) {
        this.commands.push(presetSelect.value);
        presetSelect.selectedIndex = 0;
        this.renderCmds(container);
      }
    });

    this.renderKwRules(container);
    container.querySelector("#kw-add-rule")?.addEventListener("click", () => {
      this.rules.push({ pattern: "", command: "", case_sensitive: false });
      this.renderKwRules(container);
    });
  }

  /** Render the integration-command rows from config and wire their inputs. */
  private renderCmds(container: HTMLElement) {
    const list = container.querySelector<HTMLElement>("#hook-cmd-list");
    if (!list) return;
    if (this.commands.length === 0) {
      list.innerHTML = `<span style="font-size: 11px; color: var(--fg-faded);">No integration scripts yet — add one above, or pick a preset.</span>`;
      return;
    }
    list.innerHTML = this.commands
      .map(
        (cmd, i) => `
        <div class="hook-cmd-row" data-idx="${i}" style="display: flex; gap: 8px; align-items: center;">
          <span style="font-size: 11px; color: var(--fg-faded); width: 14px; text-align: right;">${i + 1}.</span>
          <input class="hook-cmd" type="text" placeholder="Command to run…" value="${escapeAttr(cmd)}"
            style="flex: 1; min-width: 0; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 5px 8px; font-size: 12px; color: var(--fg-default);" />
          <button class="inline-button hook-browse" title="Browse for a script" style="white-space: nowrap;">Browse…</button>
          <button class="inline-button hook-test" title="Run this command once with sample data" style="white-space: nowrap;">Test</button>
          <button class="inline-button hook-remove" title="Remove command" style="padding: 4px 8px;">✕</button>
        </div>`,
      )
      .join("");

    list.querySelectorAll<HTMLElement>(".hook-cmd-row").forEach((row) => {
      const idx = Number(row.dataset.idx);
      const input = row.querySelector<HTMLInputElement>(".hook-cmd");
      input?.addEventListener("input", (e) => {
        this.commands[idx] = (e.target as HTMLInputElement).value;
      });
      row.querySelector(".hook-browse")?.addEventListener("click", async () => {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const path = await open({ multiple: false });
        if (typeof path === "string" && input) {
          // Quote the path if it contains spaces — the daemon splits with shlex.
          input.value = path.includes(" ") ? `"${path}"` : path;
          this.commands[idx] = input.value;
        }
      });
      row.querySelector(".hook-test")?.addEventListener("click", async () => {
        const el = container.querySelector<HTMLElement>("#hook-result")!;
        el.style.display = "block";
        el.className = "test-result";
        el.textContent = `Running command ${idx + 1}…`;
        const custom_command = input ? input.value : undefined;
        const result = await invoke<{ ok: boolean; message: string }>("wizard_test_hook", {
          customCommand: custom_command,
        }).catch((e) => ({ ok: false, message: String(e) }));
        el.className = `test-result ${result.ok ? "ok" : "err"}`;
        el.textContent = result.message;
      });
      row.querySelector(".hook-remove")?.addEventListener("click", () => {
        this.commands.splice(idx, 1);
        this.renderCmds(container);
      });
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
