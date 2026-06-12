import { bindFieldEvents, renderField } from "./form";

/**
 * Dictation (transcription-in-place) settings — the fast lane.
 *
 * By default an in-place dictation skips the queue and the full pipeline:
 * transcribe with a fast provider → instant rule-based polish → type at the
 * cursor, with the library save happening afterwards in the background. This
 * section tunes that behavior; the dedicated STT provider (`in_place.stt`)
 * follows the live preview's fast model automatically and gets its own picker
 * with the unified model-selection work.
 */
export class SectionInPlace {
  private config: any;

  constructor(container: HTMLElement, config: any) {
    this.config = config;
    this.render(container);
  }

  private render(container: HTMLElement) {
    if (!this.config.in_place) {
      this.config.in_place = {
        cleanup: "fast",
        full_pipeline: false,
        save_to_library: true,
        type_mode: "type",
      };
    }
    const ip = this.config.in_place;

    container.innerHTML = `
      <div class="settings-section">
        <h3>Dictation (in-place)</h3>
        <p style="font-size: 12px; color: var(--fg-muted); margin: 0 0 12px; line-height: 1.5;">
          The in-place hotkey types what you say straight into the focused window.
          Dictations take a <b>fast lane</b>: they skip the processing queue and the
          full pipeline, so the text lands in well under a second — even while a
          meeting is transcribing. The STT model follows the Live Preview's fast
          model when that's enabled, else the main transcription provider.
        </p>

        <div class="settings-field">
          <label>Text polish</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ip-cleanup">
              <option value="fast" ${(ip.cleanup ?? "fast") === "fast" ? "selected" : ""}>Fast — instant, rule-based (recommended)</option>
              <option value="off" ${ip.cleanup === "off" ? "selected" : ""}>Off — raw transcription</option>
              <option value="llm" ${ip.cleanup === "llm" ? "selected" : ""}>AI cleanup — slower, full LLM pass</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              <b>Fast</b> strips filler words ("um", "uh") and whisper's non-speech tags, fixes
              stutter-doubled words, capitalization, and end punctuation — with zero added latency.
              <b>AI cleanup</b> runs the Post-Processing provider before typing, adding its full
              round-trip time to every dictation.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Insert text by</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ip-type-mode">
              <option value="type" ${(ip.type_mode ?? "type") === "type" ? "selected" : ""}>Typing — simulated keystrokes</option>
              <option value="paste" ${ip.type_mode === "paste" ? "selected" : ""}>Pasting — clipboard + Ctrl+V</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Typing works everywhere but takes a moment for long text. Pasting is near-instant —
              your previous clipboard is put back afterwards — but a few apps block paste.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Keep dictations in the library</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "in_place.save_to_library", label: "", kind: "checkbox" },
              ip.save_to_library ?? true,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              On: after the text is typed, the recording saves like any other (searchable, with
              audio). Off: dictations are ephemeral — audio and transcript are discarded once typed.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Run the full pipeline</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "in_place.full_pipeline", label: "", kind: "checkbox" },
              ip.full_pipeline ?? false,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Route dictations through the normal queue and every configured step (cleanup,
              summary, auto-tags, hooks) <b>before</b> typing — the pre-fast-lane behavior. Slow;
              only useful when dictations must trigger the same automation as recordings.
            </span>
          </div>
        </div>
      </div>
    `;

    bindFieldEvents(container, this.config);
    container
      .querySelector<HTMLSelectElement>("#ip-cleanup")
      ?.addEventListener("change", (e) => {
        this.config.in_place.cleanup = (e.target as HTMLSelectElement).value;
      });
    container
      .querySelector<HTMLSelectElement>("#ip-type-mode")
      ?.addEventListener("change", (e) => {
        this.config.in_place.type_mode = (e.target as HTMLSelectElement).value;
      });
  }
}
