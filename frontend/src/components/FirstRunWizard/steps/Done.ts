import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../../../utils/toast";
import type { StepCallbacks } from "./Welcome";

export class Done {
  constructor(
    body: HTMLElement,
    footer: HTMLElement,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    _config: any,
    cbs: StepCallbacks,
  ) {
    body.innerHTML = `
      <h2 class="wizard-title">You're set up</h2>
      <p class="wizard-subtitle">Try saying something now.</p>
      <button class="wizard-record-big" id="record">●</button>
    `;
    footer.innerHTML = `
      <button class="wizard-btn" id="back">← Back</button>
      <span class="spacer"></span>
      <button class="wizard-btn primary" id="finish">Finish</button>
    `;
    body.querySelector("#record")?.addEventListener("click", async () => {
      try {
        await invoke("record_start", { mode: "oneshot" });
      } catch (e) {
        showToast(`Failed to start recording: ${e}`, "error");
      }
    });
    footer.querySelector("#back")?.addEventListener("click", () => cbs.onBack());
    // onFinish persists the config and closes the wizard (see index.ts).
    footer.querySelector("#finish")?.addEventListener("click", () => cbs.onFinish());
  }
}
