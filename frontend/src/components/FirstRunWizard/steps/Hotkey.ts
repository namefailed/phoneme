import type { StepCallbacks } from "./Welcome";

export class Hotkey {
  private capturing = false;
  private keydownHandler: ((e: KeyboardEvent) => void) | null = null;

  constructor(
    body: HTMLElement,
    footer: HTMLElement,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private config: any,
    cbs: StepCallbacks,
  ) {
    body.innerHTML = `
      <h2 class="wizard-title">Hotkey (optional)</h2>
      <p class="wizard-subtitle">Want a global hotkey for hold-to-talk? If you use Kanata/AHK to bind this externally, leave it off.</p>
      <div class="wizard-field">
        <label><input type="checkbox" id="enabled" ${
          this.config.hotkey.enabled ? "checked" : ""
        } /> Enable global hotkey</label>
      </div>
      <div class="wizard-field">
        <label>Combo</label>
        <div>
          <button type="button" class="combo-capture" id="capture">
            <span id="combo-display">${escapeHtml(
              this.config.hotkey.combo || "(press a combo)",
            )}</span>
          </button>
          <span class="help" id="capture-hint">Click, then press your desired combo.</span>
        </div>
      </div>
    `;
    footer.innerHTML = `
      <button class="wizard-btn" id="back">← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn" id="skip">Skip setup</button>
      <button class="wizard-btn primary" id="next">Continue →</button>
    `;

    body.querySelector<HTMLInputElement>("#enabled")!.addEventListener("change", (e) => {
      this.config.hotkey.enabled = (e.target as HTMLInputElement).checked;
    });

    body.querySelector<HTMLButtonElement>("#capture")!.addEventListener("click", () => {
      this.startCapture(body);
    });

    footer.querySelector("#back")?.addEventListener("click", () => {
      this.stopCapture(body);
      cbs.onBack();
    });
    footer.querySelector("#skip")?.addEventListener("click", () => {
      this.stopCapture(body);
      cbs.onSkip();
    });
    footer.querySelector("#next")?.addEventListener("click", () => {
      this.stopCapture(body);
      cbs.onNext();
    });
  }

  private startCapture(body: HTMLElement) {
    if (this.capturing) return;
    this.capturing = true;
    const btn = body.querySelector<HTMLButtonElement>("#capture")!;
    const display = body.querySelector<HTMLElement>("#combo-display")!;
    const hint = body.querySelector<HTMLElement>("#capture-hint")!;
    btn.classList.add("capturing");
    display.textContent = "(press now…)";
    hint.textContent = "Esc to cancel.";

    this.keydownHandler = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        this.stopCapture(body);
        return;
      }
      // Ignore lone modifier-key presses; wait for the "real" key.
      if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return;

      const parts: string[] = [];
      if (e.ctrlKey) parts.push("Ctrl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      if (e.metaKey) parts.push("Meta");
      // e.code is more stable (`Space`, `KeyA`) than e.key. Prefer e.code for
      // letters/digits, fall back to e.key for symbols.
      const keyName = e.code.startsWith("Key")
        ? e.code.slice(3)
        : e.code.startsWith("Digit")
          ? e.code.slice(5)
          : e.code === "Space"
            ? "Space"
            : e.key.length === 1
              ? e.key.toUpperCase()
              : e.key;
      parts.push(keyName);

      const combo = parts.join("+");
      this.config.hotkey.combo = combo;
      display.textContent = combo;
      this.stopCapture(body);
    };
    document.addEventListener("keydown", this.keydownHandler, { capture: true });
  }

  private stopCapture(body?: HTMLElement) {
    if (!this.capturing) return;
    this.capturing = false;
    if (this.keydownHandler) {
      document.removeEventListener("keydown", this.keydownHandler, { capture: true });
      this.keydownHandler = null;
    }
    if (body) {
      const btn = body.querySelector<HTMLButtonElement>("#capture");
      const hint = body.querySelector<HTMLElement>("#capture-hint");
      btn?.classList.remove("capturing");
      if (hint) hint.textContent = "Click, then press your desired combo.";
    }
  }
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
