import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

/**
 * Settings → Recording: capture options under `config.recording` — the
 * input device (live-listed via the `list_input_devices` command), the
 * capture source, audio directory, max duration, silence auto-stop with its
 * threshold/window, and the pre-roll buffer. Plain section class on the
 * form.ts binding; the daemon picks changes up when the saved config
 * reloads.
 *
 * Renders SYNCHRONOUSLY (the device list fills in afterward) so the Capture tab
 * appears all at once — awaiting `list_input_devices` before the first paint
 * left this top section blank while the rest of the tab was already on screen.
 */
export class SectionRecording {
  private container: HTMLElement;

  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    this.container = container;
    // Immediate paint with just the saved device + the system-default option;
    // the full device list is appended once the IPC returns (see loadDevices).
    container.innerHTML = this.markup();
    bindFieldEvents(container, this.config);
    void this.loadDevices();
  }

  /** Initial microphone options: system default, plus the saved device (so it
   *  shows selected before the full list loads). The rest arrive in loadDevices. */
  private deviceOptions(): { value: string; label: string }[] {
    const opts = [{ value: "default", label: "(system default)" }];
    const saved = this.config.recording.input_device;
    if (saved && saved !== "default") opts.push({ value: saved, label: saved });
    return opts;
  }

  private markup(): string {
    return `
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
                options: this.deviceOptions(),
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
          <label>Auto-stop on silence</label>
          <div>${renderField(
            { key: "recording.auto_stop_on_silence", label: "", kind: "checkbox" },
            this.config.recording.auto_stop_on_silence || false,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            When <b>on</b>, the Record button stops automatically once your mic goes quiet (using the threshold and window below) — good for hands-free quick notes.<br/>
            When <b>off</b> (default), the Record button is a <b>Start/Stop toggle</b>: it records until you click stop, so a quiet mic or a natural pause never cuts you off. The silence threshold and window below only apply when this is on. (Your push-to-talk hotkey is unaffected.)
          </span>
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
          <label>Normalize audio level</label>
          <div>${renderField(
            { key: "recording.normalize", label: "", kind: "checkbox" },
            this.config.recording.normalize || false,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            Boost quiet recordings to a consistent level before transcribing — a turned-down mic still hands transcription a healthy signal.<br/>
            <b>Off</b> by default; affects newly captured recordings only (not the live preview or imported files).
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
  }

  /** Append the live input-device list to the already-rendered Microphone select
   *  (best-effort). Runs after the synchronous paint so it never delays the tab. */
  private async loadDevices() {
    const devices: string[] = await invoke<string[]>("list_input_devices").catch(() => []);
    if (!devices.length) return;
    const sel = this.container.querySelector<HTMLSelectElement>(
      'select[data-key="recording.input_device"]',
    );
    if (!sel) return;
    const have = new Set([...sel.options].map((o) => o.value));
    for (const d of devices) {
      if (have.has(d)) continue;
      const opt = document.createElement("option");
      opt.value = d;
      opt.textContent = d;
      sel.appendChild(opt);
    }
  }
}
