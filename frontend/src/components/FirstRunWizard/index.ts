import { invoke } from "@tauri-apps/api/core";
import { Welcome } from "./steps/Welcome";
import { ModePicker } from "./steps/ModePicker";
import { ConfigureMode } from "./steps/ConfigureMode";
import { Microphone } from "./steps/Microphone";
import { Hook } from "./steps/Hook";
import { Hotkey } from "./steps/Hotkey";
import { Done } from "./steps/Done";
import "./styles.css";

export type WizardStep =
  | "welcome"
  | "mode"
  | "configure"
  | "mic"
  | "hook"
  | "hotkey"
  | "done";

export class FirstRunWizard {
  private container: HTMLElement;
  private onComplete: () => void;
  private step: WizardStep = "welcome";
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private config: any;

  constructor(container: HTMLElement, onComplete: () => void) {
    this.container = container;
    this.onComplete = onComplete;
    void this.init();
  }

  private async init() {
    this.config = await invoke("read_config"); // defaults if no file exists
    this.render();
  }

  private render() {
    this.container.innerHTML = `
      <div class="wizard-shell">
        <div class="wizard-header">
          <div class="wizard-brand">🎙 Phoneme — Setup</div>
          <div class="wizard-dots" id="wizard-dots"></div>
        </div>
        <div class="wizard-body" id="wizard-body"></div>
        <div class="wizard-footer" id="wizard-footer"></div>
      </div>
    `;
    this.renderDots();
    this.renderStep();
  }

  private renderDots() {
    const all: WizardStep[] = [
      "welcome",
      "mode",
      "configure",
      "mic",
      "hook",
      "hotkey",
      "done",
    ];
    const idx = all.indexOf(this.step);
    const dots = this.container.querySelector("#wizard-dots")!;
    dots.innerHTML = all
      .map((s, i) => {
        const klass = i < idx ? "done" : i === idx ? "active" : "";
        return `<span class="wizard-dot ${klass}" title="${s}"></span>`;
      })
      .join("");
  }

  private renderStep() {
    const body = this.container.querySelector<HTMLElement>("#wizard-body")!;
    const footer = this.container.querySelector<HTMLElement>("#wizard-footer")!;
    const cbs = {
      onNext: () => this.go("next"),
      onBack: () => this.go("back"),
      onSkip: () => void this.skip(),
      onFinish: () => void this.finish(),
    };
    switch (this.step) {
      case "welcome":
        new Welcome(body, footer, this.config, cbs);
        break;
      case "mode":
        new ModePicker(body, footer, this.config, cbs);
        break;
      case "configure":
        new ConfigureMode(body, footer, this.config, cbs);
        break;
      case "mic":
        new Microphone(body, footer, this.config, cbs);
        break;
      case "hook":
        new Hook(body, footer, this.config, cbs);
        break;
      case "hotkey":
        new Hotkey(body, footer, this.config, cbs);
        break;
      case "done":
        new Done(body, footer, this.config, cbs);
        break;
    }
  }

  private go(direction: "next" | "back") {
    const all: WizardStep[] = [
      "welcome",
      "mode",
      "configure",
      "mic",
      "hook",
      "hotkey",
      "done",
    ];
    const idx = all.indexOf(this.step);
    const next =
      direction === "next"
        ? Math.min(idx + 1, all.length - 1)
        : Math.max(idx - 1, 0);
    this.step = all[next];
    this.renderDots();
    this.renderStep();
  }

  private async skip() {
    // Persist whatever defaults we have and exit.
    await invoke("write_config", { config: this.config });
    this.onComplete();
  }

  private async finish() {
    await invoke("write_config", { config: this.config });
    this.onComplete();
  }

  dispose() {}
}
