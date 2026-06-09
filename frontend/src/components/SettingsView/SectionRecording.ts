import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

export class SectionRecording {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    void this.render(container);
  }

  private async render(container: HTMLElement) {
    const devices: string[] = await invoke<string[]>("list_input_devices").catch(() => []);
    container.innerHTML = `
      <div class="settings-section">
        <h3>Recording</h3>
        <div class="settings-field long-input">
          <label>Microphone</label>
          <div>
            ${renderField(
              {
                key: "recording.input_device",
                label: "",
                kind: "select",
                options: [{ value: "default", label: "(system default)" }].concat(
                  devices.map((d) => ({ value: d, label: d })),
                ),
              },
              this.config.recording.input_device,
            )}
          </div>
        </div>
        <div class="settings-field">
          <label>Audio source</label>
          <div>
            ${renderField(
              {
                key: "recording.source",
                label: "",
                kind: "select",
                options: [
                  { value: "microphone", label: "Microphone" },
                  { value: "system_audio", label: "System audio (loopback) — Windows" },
                ],
              },
              this.config.recording.source || "microphone",
            )}
          </div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            <b>System audio</b> records what's playing through your speakers (meetings, videos) via WASAPI loopback. Windows only.
          </span>
        </div>
        <div class="settings-field long-input">
          <label>Audio directory</label>
          <div>${renderField(
            { key: "recording.audio_dir", label: "", kind: "text" },
            this.config.recording.audio_dir,
          )}</div>
        </div>
        <div class="settings-field">
          <label>Max duration (seconds)</label>
          <div>${renderField(
            { key: "recording.max_duration_secs", label: "", kind: "number" },
            this.config.recording.max_duration_secs,
          )}</div>
        </div>
        <div class="settings-field">
          <label>Silence threshold (dBFS)</label>
          <div>${renderField(
            { key: "recording.silence_threshold_dbfs", label: "", kind: "number" },
            this.config.recording.silence_threshold_dbfs,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            The volume level (in decibels) below which audio is considered "silence".<br/>
            <b>-45 dBFS</b> is good for quiet rooms. Use <b>-30 dBFS</b> for noisy environments to prevent background noise from keeping the recording open.
          </span>
        </div>
        <div class="settings-field">
          <label>Silence window (ms)</label>
          <div>${renderField(
            { key: "recording.silence_window_ms", label: "", kind: "number" },
            this.config.recording.silence_window_ms,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            How long you must pause (in milliseconds) before Phoneme considers you finished speaking and automatically stops the recording. (e.g. 1500 = 1.5 seconds)
          </span>
        </div>
        <div class="settings-field">
          <label>Pre-roll (ms)</label>
          <div>${renderField(
            { key: "recording.pre_roll_ms", label: "", kind: "number" },
            this.config.recording.pre_roll_ms ?? 0,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            Captures up to this many milliseconds of audio from <b>before</b> you hit record, so the first syllable isn't clipped. (e.g. 500 = 0.5 seconds)<br/>
            <b>0 disables it</b> (default). When set above 0, Phoneme keeps your <b>microphone open continuously</b> between recordings, holding the most recent audio in a rolling in-memory buffer that is constantly discarded. Nothing is written to disk unless you actually start a recording. Microphone source only — ignored for system audio.
          </span>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);
  }
}
