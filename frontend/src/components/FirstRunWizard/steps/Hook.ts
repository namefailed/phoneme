import type { StepCallbacks } from "./Welcome";

export class Hook {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    body: HTMLElement,
    footer: HTMLElement,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private config: any,
    cbs: StepCallbacks,
  ) {
    body.innerHTML = `
      <h2 class="wizard-title">Hook (delivery)</h2>
      <p class="wizard-subtitle">Phoneme runs this script with the transcript as JSON on stdin. Default writes to stdout.</p>
      <div class="wizard-field">
        <label>Hook command</label>
        <input type="text" id="cmd" value="${this.config.hook.commands?.[0] || ""}" />
      </div>
      <div class="wizard-field">
        <label>Timeout (seconds)</label>
        <input type="number" id="to" value="${this.config.hook.timeout_secs}" />
      </div>
    `;
    footer.innerHTML = `
      <button class="wizard-btn" id="back">← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn" id="skip">Skip setup</button>
      <button class="wizard-btn primary" id="next">Continue →</button>
    `;
    body.querySelector<HTMLInputElement>("#cmd")!.addEventListener("input", (e) => {
      this.config.hook.commands = [(e.target as HTMLInputElement).value];
    });
    body.querySelector<HTMLInputElement>("#to")!.addEventListener("input", (e) => {
      this.config.hook.timeout_secs = Number((e.target as HTMLInputElement).value);
    });
    footer.querySelector("#back")?.addEventListener("click", () => cbs.onBack());
    footer.querySelector("#skip")?.addEventListener("click", () => cbs.onSkip());
    footer.querySelector("#next")?.addEventListener("click", () => cbs.onNext());
  }
}
