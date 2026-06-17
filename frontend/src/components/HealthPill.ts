import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { subscribeHealth, startHealthPolling, type HealthSnapshot } from "../state/health";

/**
 * The app-health pill (green = all Doctor checks pass, red + count = something
 * needs attention; click opens the Doctor). Used in both the header bar and the
 * Settings page, reading the shared {@link subscribeHealth} store so a single
 * Doctor poll feeds every instance. Light DOM so the global `.hb-health` styles
 * (health-pill.css) apply; fixed-width so it never resizes as health resolves.
 */
@customElement("ph-health-pill")
export class HealthPillElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM — shares the global pill styles.
  }

  @state() private snap: HealthSnapshot = { level: "unknown", issues: [] };
  private unsub?: () => void;

  connectedCallback() {
    super.connectedCallback();
    startHealthPolling();
    this.unsub = subscribeHealth((s) => {
      this.snap = s;
    });
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.unsub?.();
    this.unsub = undefined;
  }

  private async open() {
    const { openDoctor } = await import("./DoctorModal");
    await openDoctor();
  }

  render() {
    const { level, issues } = this.snap;
    const title =
      level === "bad"
        ? `Problems found: ${issues.map((i) => i.name).join(", ")} — click to open Doctor`
        : level === "ok"
          ? "All systems healthy — click to open Doctor"
          : "Checking health…";
    return html`
      <button class="hb-health ${level}" title=${title} aria-label="App health" @click=${this.open}>
        <span class="hb-health-dot" aria-hidden="true"></span>${level === "bad"
          ? html`<span class="hb-health-n">${issues.length}</span>`
          : ""}
      </button>
    `;
  }
}
