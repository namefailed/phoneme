import { invoke } from "@tauri-apps/api/core";
import type { StepCallbacks } from "./Welcome";

export class Microphone {
  constructor(
    body: HTMLElement,
    footer: HTMLElement,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private config: any,
    cbs: StepCallbacks,
  ) {
    void this.render(body, footer, cbs);
  }

  private async render(body: HTMLElement, footer: HTMLElement, cbs: StepCallbacks) {
    const devices: string[] = await invoke<string[]>("list_input_devices").catch(
      () => [],
    );
    body.innerHTML = `
      <h2 class="wizard-title">Microphone</h2>
      <p class="wizard-subtitle">Pick the input device Phoneme should record from.</p>
      <div class="wizard-field">
        <label>Device</label>
        <select id="dev">
          <option value="default">(system default)</option>
          ${devices.map((d) => `<option value="${d}">${d}</option>`).join("")}
        </select>
      </div>
    `;
    this.renderFooter(footer, cbs);
    const dev = body.querySelector<HTMLSelectElement>("#dev")!;
    dev.value = this.config.recording.input_device;
    dev.addEventListener("change", (e) => {
      this.config.recording.input_device = (e.target as HTMLSelectElement).value;
    });
  }

  private renderFooter(footer: HTMLElement, cbs: StepCallbacks) {
    footer.innerHTML = `
      <button class="wizard-btn" id="back">← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn" id="skip">Skip setup</button>
      <button class="wizard-btn primary" id="next">Continue →</button>
    `;
    footer.querySelector("#back")?.addEventListener("click", () => cbs.onBack());
    footer.querySelector("#skip")?.addEventListener("click", () => cbs.onSkip());
    footer.querySelector("#next")?.addEventListener("click", () => cbs.onNext());
  }
}
