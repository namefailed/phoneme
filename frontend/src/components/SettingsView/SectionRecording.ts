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
    // The numeric fields are bound by hand (not via data-key/form.ts) so they
    // clamp to sane ranges and never write NaN when cleared mid-edit.
    this.bindNumbers();
    void this.loadDevices();
  }

  /** Wire the numeric Recording fields with min/max clamps and a NaN guard.
   *  form.ts' generic binding does `Number(input.value)`, which is NaN for an
   *  emptied field (a normal editing step) — that would serialize to a bad
   *  value on Save. These bespoke handlers (mirroring SectionPreview) clamp to
   *  range and fall back to the last good value when the input is blank/NaN. */
  private bindNumbers() {
    const wire = (
      id: string,
      key: string,
      min: number,
      max: number,
      round: boolean,
    ) => {
      const el = this.container.querySelector<HTMLInputElement>(`#${id}`);
      el?.addEventListener("change", () => {
        const n = Number(el.value);
        if (Number.isFinite(n)) {
          const v = Math.min(max, Math.max(min, round ? Math.round(n) : n));
          this.config.recording[key] = v;
          // Reflect the clamped value back so the field shows what was stored.
          el.value = String(v);
        } else {
          // Cleared/invalid — keep the previous value and restore the display.
          el.value = String(this.config.recording[key] ?? min);
        }
      });
    };
    // u32 fields: non-negative whole numbers. dBFS is a level <= 0.
    wire("rec-max-duration", "max_duration_secs", 0, 86400, true);
    wire("rec-silence-dbfs", "silence_threshold_dbfs", -120, 0, false);
    wire("rec-silence-window", "silence_window_ms", 0, 600000, true);
    wire("rec-pre-roll", "pre_roll_ms", 0, 60000, true);
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
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
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
          <div><input type="number" id="rec-max-duration" min="0" max="86400" step="1" value="${
            this.config.recording.max_duration_secs ?? 0
          }" /></div>
        </div>
        <div class="settings-field">
          <label>Auto-stop on silence</label>
          <div>${renderField(
            { key: "recording.auto_stop_on_silence", label: "", kind: "checkbox" },
            this.config.recording.auto_stop_on_silence || false,
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            When <b>on</b>, the Record button stops automatically once your mic goes quiet (using the threshold and window below) — good for hands-free quick notes.<br/>
            When <b>off</b> (default), the Record button is a <b>Start/Stop toggle</b>: it records until you click stop, so a quiet mic or a natural pause never cuts you off. The silence threshold and window below only apply when this is on. (Your push-to-talk hotkey is unaffected.)
          </span>
        </div>
        <div class="settings-field">
          <label>Silence threshold (dBFS)</label>
          <div><input type="number" id="rec-silence-dbfs" min="-120" max="0" step="1" value="${
            this.config.recording.silence_threshold_dbfs ?? -45
          }" /></div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            The volume level (in decibels) below which audio is considered "silence".<br/>
            <b>-45 dBFS</b> is good for quiet rooms. Use <b>-30 dBFS</b> for noisy environments to prevent background noise from keeping the recording open.
          </span>
        </div>
        <div class="settings-field">
          <label>Silence window (ms)</label>
          <div><input type="number" id="rec-silence-window" min="0" max="600000" step="100" value="${
            this.config.recording.silence_window_ms ?? 3000
          }" /></div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            How long you must pause (in milliseconds) before Phoneme considers you finished speaking and automatically stops the recording. (e.g. 1500 = 1.5 seconds)
          </span>
        </div>
        <div class="settings-field">
          <label>Normalize audio level</label>
          <div>${renderField(
            { key: "recording.normalize", label: "", kind: "checkbox" },
            this.config.recording.normalize || false,
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            Boost quiet recordings to a consistent level before transcribing — a turned-down mic still hands transcription a healthy signal.<br/>
            <b>Off</b> by default; affects newly captured recordings only (not the live preview or imported files).
          </span>
        </div>
        <div class="settings-field">
          <label>Pre-roll (ms)</label>
          <div><input type="number" id="rec-pre-roll" min="0" max="60000" step="100" value="${
            this.config.recording.pre_roll_ms ?? 0
          }" /></div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
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
