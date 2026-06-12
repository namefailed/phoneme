import { errText } from "../../utils/error";
import { renderField, bindFieldEvents } from "./form";
import { listRecordings, IMPORT_AUDIO_EXTENSIONS } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { pickAndImportAudio } from "../../utils/import";

export class SectionStorage {
   
  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Storage</h3>
        <div class="settings-field">
          <label>Audio directory</label>
          <div>
            ${renderField(
              { key: "recording.audio_dir", label: "", kind: "text" },
              this.config.recording.audio_dir,
            )}
            <button class="inline-button" id="pick-audio-dir">Browse…</button>
            <button class="inline-button" id="open-audio-dir">Open</button>
          </div>
        </div>

        <div class="settings-field">
          <label>Max age (days)</label>
          <div>
            <input type="number" min="1" id="ret-max-age" placeholder="disabled"
              style="max-width: 120px;" value="${this.config.retention?.max_age_days ?? ""}" />
          </div>
          <span>Auto-delete recordings older than this. Leave blank to disable.</span>
        </div>

        <div class="settings-field">
          <label>Max recordings</label>
          <div>
            <input type="number" min="1" id="ret-max-count" placeholder="disabled"
              style="max-width: 120px;" value="${this.config.retention?.max_count ?? ""}" />
          </div>
          <span>Keep only the most recent N recordings. Leave blank to disable.</span>
        </div>

        <div class="settings-field">
          <label>Delete audio only</label>
          <div>
            <input type="checkbox" class="toggle-switch" id="ret-delete-audio" ${this.config.retention?.delete_audio ? "checked" : ""} />
          </div>
          <span>When pruning, remove the audio file but keep the transcript. Auto-delete runs on
            startup and hourly; only completed recordings are affected — in-progress ones are always preserved.</span>
        </div>

        <div class="settings-field">
          <label>Import audio</label>
          <div>
            <button class="inline-button" id="btn-import-audio">⬆ Import audio…</button>
          </div>
          <span>Bring existing audio files into Phoneme to transcribe and process them.
            Supported formats: ${(IMPORT_AUDIO_EXTENSIONS as readonly string[]).join(", ")}.</span>
        </div>

        <div class="settings-field">
          <label>Export recordings</label>
          <div>
            <select id="export-format" style="max-width: 110px;">
              <option value="json">JSON</option>
              <option value="csv">CSV</option>
              <option value="txt">Plain Text</option>
            </select>
            <button class="inline-button" id="btn-export-all">⬇ Export All…</button>
            <span id="export-status" style="font-size:11px; color: var(--fg-muted);"></span>
          </div>
          <span>Exports all recordings and their transcripts to a single file.</span>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    // Retention fields — optional numbers map blank → null, filled → number.
    const retAgeEl = container.querySelector<HTMLInputElement>("#ret-max-age");
    const retCountEl = container.querySelector<HTMLInputElement>("#ret-max-count");
    const retAudioEl = container.querySelector<HTMLInputElement>("#ret-delete-audio");
    const ensureRetention = () => {
      if (!this.config.retention) this.config.retention = {};
    };
    if (retAgeEl) {
      retAgeEl.addEventListener("input", () => {
        ensureRetention();
        const v = retAgeEl.value.trim();
        this.config.retention.max_age_days = v === "" ? null : Math.max(1, parseInt(v, 10));
      });
    }
    if (retCountEl) {
      retCountEl.addEventListener("input", () => {
        ensureRetention();
        const v = retCountEl.value.trim();
        this.config.retention.max_count = v === "" ? null : Math.max(1, parseInt(v, 10));
      });
    }
    if (retAudioEl) {
      retAudioEl.addEventListener("change", () => {
        ensureRetention();
        this.config.retention.delete_audio = retAudioEl.checked;
      });
    }

    container
      .querySelector("#pick-audio-dir")
      ?.addEventListener("click", async () => {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const dir = await open({ directory: true, multiple: false });
        if (typeof dir === "string") {
          const input = container.querySelector<HTMLInputElement>(
            `[data-key="recording.audio_dir"]`,
          )!;
          input.value = dir;
          this.config.recording.audio_dir = dir;
        }
      });

    container
      .querySelector("#open-audio-dir")
      ?.addEventListener("click", async () => {
        const { open } = await import("@tauri-apps/plugin-shell");
        await open(this.config.recording.audio_dir).catch(() => {});
      });

    container
      .querySelector("#btn-import-audio")
      ?.addEventListener("click", async () => {
        await pickAndImportAudio();
      });

    container
      .querySelector("#btn-export-all")
      ?.addEventListener("click", async () => {
        const formatEl = container.querySelector<HTMLSelectElement>("#export-format");
        const statusEl = container.querySelector<HTMLElement>("#export-status");
        const format = formatEl?.value ?? "json";
        if (statusEl) statusEl.textContent = "Loading recordings…";
        try {
          const recordings = await listRecordings({ limit: 10000 });
          let content = "";
          const ext = format;
          if (format === "json") {
            content = JSON.stringify(recordings, null, 2);
          } else if (format === "csv") {
            const header = "id,started_at,duration_ms,status,model,transcript";
            const rows = recordings.map(r => [
              r.id,
              r.started_at,
              r.duration_ms,
              r.status,
              r.model ?? "",
              JSON.stringify(r.transcript ?? ""),
            ].join(","));
            content = [header, ...rows].join("\n");
          } else {
            // Plain text: one recording per block
            content = recordings.map(r =>
              `[${r.started_at}] ${r.id}\nStatus: ${r.status}\nDuration: ${r.duration_ms}ms\n\n${r.transcript ?? "(no transcript)"}\n\n${"─".repeat(60)}`
            ).join("\n");
          }

          const { save } = await import("@tauri-apps/plugin-dialog");
          const { writeTextFile } = await import("@tauri-apps/plugin-fs");
          const dest = await save({
            defaultPath: `phoneme-export.${ext}`,
            filters: [{ name: ext.toUpperCase(), extensions: [ext] }, { name: "All files", extensions: ["*"] }],
          });
          if (dest) {
            await writeTextFile(dest, content);
            showToast(`Exported ${recordings.length} recordings`, "success");
            if (statusEl) statusEl.textContent = `Exported ${recordings.length} recordings.`;
          } else {
            if (statusEl) statusEl.textContent = "";
          }
        } catch (e) {
          showToast(`Export failed: ${errText(e)}`, "error");
          if (statusEl) statusEl.textContent = "";
        }
      });
  }
}
