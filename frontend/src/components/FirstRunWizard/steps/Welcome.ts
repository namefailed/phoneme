export type StepCallbacks = {
  onNext: () => void;
  onBack: () => void;
  onSkip: () => void;
  onFinish: () => void;
};

export class Welcome {
  constructor(
    body: HTMLElement,
    footer: HTMLElement,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    _config: any,
    cbs: StepCallbacks,
  ) {
    body.innerHTML = `
      <h2 class="wizard-title">Welcome to Phoneme</h2>
      <p class="wizard-subtitle">Local-first voice notes. Press a hotkey, speak, get a transcript — all on your machine.</p>
      <ul class="wizard-bullets">
        <li>Records audio via your microphone</li>
        <li>Transcribes locally with whisper-server (no cloud)</li>
        <li>Emits the transcript as JSON to your hook script</li>
      </ul>
      <p class="wizard-subtitle">Let's get it set up.</p>
    `;
    footer.innerHTML = `
      <span class="spacer"></span>
      <button class="wizard-btn primary" id="next">Continue →</button>
    `;
    footer.querySelector("#next")?.addEventListener("click", () => cbs.onNext());
  }
}
