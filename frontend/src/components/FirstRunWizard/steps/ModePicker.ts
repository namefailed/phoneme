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
      <h2 class="wizard-title">What should Phoneme set up?</h2>
      <p class="wizard-subtitle">We can automatically download the AI models required to run locally. Pick an option — you can always change this later in Settings.</p>
      <div class="mode-cards" style="grid-template-columns: 1fr 1fr;">
        <div class="mode-card" data-mode="none">
          <div class="mode-icon">🛠</div>
          <div class="mode-name">Set it up yourself</div>
          <div class="mode-desc">I already have my own Whisper and/or LLM endpoints. Don't download anything.</div>
        </div>
        <div class="mode-card" data-mode="whisper">
          <div class="mode-icon">🎙</div>
          <div class="mode-name">Install just Whisper</div>
          <div class="mode-desc">Download a local Whisper model (Speech-to-Text).</div>
        </div>
        <div class="mode-card" data-mode="ollama">
          <div class="mode-icon">🦙</div>
          <div class="mode-name">Install just Ollama</div>
          <div class="mode-desc">Download Ollama and Llama 3.2 (LLM Post-processing).</div>
        </div>
        <div class="mode-card recommended" data-mode="both">
          <div class="mode-badge">RECOMMENDED</div>
          <div class="mode-icon">✨</div>
          <div class="mode-name">Install both</div>
          <div class="mode-desc">Get the complete local AI experience (requires ~5GB disk space).</div>
        </div>
      </div>
      <div class="semantic-search-opt-in" style="margin-top: 1.5rem; display: flex; align-items: center; gap: 0.5rem; background: var(--bg-hover); padding: 1rem; border-radius: 8px;">
        <input type="checkbox" id="semantic-search" ${config.semantic_search?.enabled ?? true ? "checked" : ""}>
        <label for="semantic-search" style="font-weight: 500; cursor: pointer;">Enable Semantic Search</label>
        <div class="mode-desc" style="font-size: 0.85em; opacity: 0.8;">(Downloads a ~90MB local ONNX embedding model)</div>
      </div>
    `;
    footer.innerHTML = `
      <button class="wizard-btn" id="back">← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn" id="skip">Skip setup</button>
      <button class="wizard-btn primary" id="next" disabled>Continue →</button>
    `;

    // Ensure _setup_mode field exists
    if (!config._setup_mode) {
      config._setup_mode = "both";
    }
    
    // Ensure semantic search config exists and sync checkbox to config
    if (!config.semantic_search) {
      config.semantic_search = { enabled: true };
    }
    const semanticCheckbox = body.querySelector<HTMLInputElement>("#semantic-search")!;
    config.semantic_search.enabled = semanticCheckbox.checked;
    semanticCheckbox.addEventListener("change", (e) => {
      config.semantic_search.enabled = (e.target as HTMLInputElement).checked;
    });

    const preselect = body.querySelector<HTMLElement>(
      `.mode-card[data-mode="${config._setup_mode}"]`,
    );
    if (preselect) {
      preselect.classList.add("selected");
      footer.querySelector<HTMLButtonElement>("#next")!.disabled = false;
    }

    body.querySelectorAll<HTMLElement>(".mode-card[data-mode]").forEach((card) => {
      card.addEventListener("click", () => {
        body
          .querySelectorAll(".mode-card")
          .forEach((c) => c.classList.remove("selected"));
        card.classList.add("selected");
        config._setup_mode = card.dataset.mode;
        footer.querySelector<HTMLButtonElement>("#next")!.disabled = false;
      });
    });

    footer.querySelector("#back")?.addEventListener("click", () => cbs.onBack());
    footer.querySelector("#skip")?.addEventListener("click", () => cbs.onSkip());
    footer.querySelector("#next")?.addEventListener("click", () => cbs.onNext());
  }
}
