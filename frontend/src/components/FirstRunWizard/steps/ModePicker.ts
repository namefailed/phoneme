import type { StepCallbacks } from "./Welcome";

export class ModePicker {
  constructor(
    body: HTMLElement,
    footer: HTMLElement,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    config: any,
    cbs: StepCallbacks,
  ) {
    body.innerHTML = `
      <h2 class="wizard-title">How do you want to run transcription?</h2>
      <p class="wizard-subtitle">Phoneme needs a whisper-server endpoint. Pick the setup that fits — you can change this later in Settings.</p>
      <div class="mode-cards">
        <div class="mode-card" data-mode="external">
          <div class="mode-icon">🔗</div>
          <div class="mode-name">Use my own server</div>
          <div class="mode-desc">I already run whisper-server. Just point Phoneme at it.</div>
        </div>
        <div class="mode-card recommended" data-mode="bundled_model">
          <div class="mode-badge">RECOMMENDED</div>
          <div class="mode-icon">📦</div>
          <div class="mode-name">Bundled server + my model</div>
          <div class="mode-desc">Phoneme runs whisper-server for you. You provide a GGUF model.</div>
        </div>
        <div class="mode-card" data-mode="bundled_download">
          <div class="mode-icon">⬇</div>
          <div class="mode-name">Download for me</div>
          <div class="mode-desc">Phoneme downloads a fast default model for you automatically.</div>
        </div>
      </div>
    `;
    footer.innerHTML = `
      <button class="wizard-btn" id="back">← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn" id="skip">Skip setup</button>
      <button class="wizard-btn primary" id="next" disabled>Continue →</button>
    `;

    // Pre-select whatever the config already has.
    const preselect = body.querySelector<HTMLElement>(
      `.mode-card[data-mode="${config.whisper.mode}"]:not(.disabled)`,
    );
    if (preselect) {
      preselect.classList.add("selected");
      footer.querySelector<HTMLButtonElement>("#next")!.disabled = false;
    }

    body
      .querySelectorAll<HTMLElement>(".mode-card[data-mode]:not(.disabled)")
      .forEach((card) => {
        card.addEventListener("click", () => {
          body
            .querySelectorAll(".mode-card")
            .forEach((c) => c.classList.remove("selected"));
          card.classList.add("selected");
          config.whisper.mode = card.dataset.mode;
          footer.querySelector<HTMLButtonElement>("#next")!.disabled = false;
        });
      });
    footer.querySelector("#back")?.addEventListener("click", () => cbs.onBack());
    footer.querySelector("#skip")?.addEventListener("click", () => cbs.onSkip());
    footer.querySelector("#next")?.addEventListener("click", () => cbs.onNext());
  }
}
