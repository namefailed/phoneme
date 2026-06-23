import { LitElement, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { findReplace, findReplaceLibrary } from "../services/ipc";
import { errText } from "../utils/error";
import { closeModalHost } from "../utils/modalAnim";

/**
 * Find / Replace modal — literal (not regex) substring replacement across a
 * transcript. Two scopes:
 *   • This recording — rewrites one recording's live transcript (the
 *     `find_replace` command). Re-flows word/segment timing + re-embeds; the
 *     original/clean baselines are preserved so it stays revertible.
 *   • Whole library — runs the same substitution over every recording's live
 *     transcript (`find_replace_library`). This rewrites and re-embeds every
 *     matching recording, so the modal shows a clear warning and requires an
 *     explicit confirm before it runs.
 *
 * The GUI front for `phoneme find-replace [<ID>] <FIND> <REPLACE>` /
 * `phoneme find-replace --library <FIND> <REPLACE>`. Opened by
 * {@link openFindReplace}: pass a recordingId to offer the recording scope
 * (the default when present); omit it for a library-only run. Built on the
 * shared `.modal-overlay` / `.modal-dialog` chrome (Esc / overlay-click / ✕
 * close, honoring `--ui-motion`). Changes arrive back at the UI via the
 * `TranscriptUpdated` daemon events, so nothing here mutates view state
 * directly — it only reports the result summary.
 */
@customElement("ph-find-replace")
export class FindReplaceElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM so the app's CSS variables + classes apply.
  }

  /** The recording whose transcript the "This recording" scope edits. Empty =
   *  library-only (the recording scope is hidden and the scope is forced to
   *  "library"). */
  @property({ type: String }) recordingId = "";

  @state() private find = "";
  @state() private replace = "";
  @state() private caseSensitive = false;
  @state() private scope: "recording" | "library" = "recording";
  @state() private busy = false;
  /** Library scope is destructive (rewrites + re-embeds every match), so it
   *  requires an explicit confirm step before it runs. */
  @state() private confirmingLibrary = false;
  /** Inline result/error line shown after a run (empty = nothing yet). */
  @state() private result = "";
  @state() private resultKind: "ok" | "err" | "" = "";

  private get hasRecording(): boolean {
    return this.recordingId.length > 0;
  }

  /** Esc closes the modal (and stops the keystroke from reaching the global
   *  keyboard layer, so it never closes the recording behind it). */
  private keyHandler = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    // First Escape on the library confirm step backs out of the confirm rather
    // than tearing down the whole modal.
    if (this.confirmingLibrary) {
      this.confirmingLibrary = false;
      return;
    }
    this.close();
  };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("keydown", this.keyHandler);
    // Library-only when no recording is in context.
    this.scope = this.hasRecording ? "recording" : "library";
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.keyHandler);
  }

  firstUpdated() {
    this.querySelector<HTMLInputElement>("#fr-find")?.focus();
  }

  private close() {
    this.dispatchEvent(new CustomEvent("resolved"));
  }

  private onOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) this.close();
  }

  private setScope(scope: "recording" | "library") {
    this.scope = scope;
    this.confirmingLibrary = false;
    this.result = "";
    this.resultKind = "";
  }

  /** Editing the find/replace terms or the case toggle invalidates a pending
   *  library confirm + any prior result — re-confirm against the new text. */
  private onTermChanged() {
    if (this.confirmingLibrary) this.confirmingLibrary = false;
    if (this.result) {
      this.result = "";
      this.resultKind = "";
    }
  }

  /** Enter in a field runs the action (stopping the keystroke from reaching the
   *  global vim/hotkey layer). Escape is left to bubble to the document
   *  keyHandler so it closes the modal even with a field focused. */
  private onFieldKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") return;
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      void this.run();
    }
  }

  private async run() {
    if (this.busy) return;
    if (!this.find) {
      this.result = "Enter the text to find.";
      this.resultKind = "err";
      this.querySelector<HTMLInputElement>("#fr-find")?.focus();
      return;
    }
    // Library scope is destructive — gate it behind an explicit confirm.
    if (this.scope === "library" && !this.confirmingLibrary) {
      this.confirmingLibrary = true;
      this.result = "";
      this.resultKind = "";
      return;
    }

    this.busy = true;
    this.result = "";
    this.resultKind = "";
    try {
      if (this.scope === "recording") {
        const { replaced } = await findReplace(
          this.recordingId,
          this.find,
          this.replace,
          this.caseSensitive,
        );
        if (replaced > 0) {
          this.result = `Replaced ${replaced} occurrence${replaced === 1 ? "" : "s"}.`;
          this.resultKind = "ok";
        } else {
          this.result = "No matches — nothing changed.";
          this.resultKind = "err";
        }
      } else {
        const r = await findReplaceLibrary(this.find, this.replace, this.caseSensitive);
        if (r.recordings_changed > 0) {
          const recs = `${r.recordings_changed} recording${r.recordings_changed === 1 ? "" : "s"}`;
          const occ = `${r.total_replacements} occurrence${r.total_replacements === 1 ? "" : "s"}`;
          let msg = `Replaced ${occ} across ${recs}.`;
          if (r.failed > 0) {
            msg += ` ${r.failed} recording${r.failed === 1 ? "" : "s"} failed to update.`;
          }
          this.result = msg;
          this.resultKind = r.failed > 0 ? "err" : "ok";
        } else {
          this.result = "No matches across the library — nothing changed.";
          this.resultKind = "err";
        }
        this.confirmingLibrary = false;
      }
    } catch (e) {
      this.result = errText(e);
      this.resultKind = "err";
      this.confirmingLibrary = false;
    } finally {
      this.busy = false;
    }
  }

  render() {
    const runLabel = this.busy
      ? "Working…"
      : this.scope === "library"
        ? this.confirmingLibrary
          ? "Yes, replace across the library"
          : "Replace across library…"
        : "Replace in this recording";
    const runDanger = this.scope === "library";

    return html`
      <div class="modal-overlay" @click=${(e: MouseEvent) => this.onOverlayClick(e)}>
        <div
          class="modal-dialog fr-dialog"
          role="dialog"
          aria-modal="true"
          aria-labelledby="fr-title"
          style="width: 440px;"
        >
          <div class="modal-header">
            <span class="modal-icon" aria-hidden="true">🔁</span>
            <h3 class="modal-title" id="fr-title">Find &amp; Replace</h3>
            <button
              class="fr-close"
              @click=${() => this.close()}
              title="Close (Esc)"
              aria-label="Close"
              style="margin-left: auto; background: none; border: none; color: var(--fg-muted); font-size: 1.1rem; cursor: pointer; line-height: 1;"
            >✕</button>
          </div>

          <div style="display: flex; flex-direction: column; gap: 12px; margin-bottom: 16px;">
            <label style="display: flex; flex-direction: column; gap: 4px; font-size: 0.8571rem; color: var(--fg-muted);">
              Find
              <input
                id="fr-find"
                type="text"
                .value=${this.find}
                placeholder="Text to find (literal, not a pattern)"
                aria-label="Text to find"
                @input=${(e: Event) => { this.find = (e.target as HTMLInputElement).value; this.onTermChanged(); }}
                @keydown=${(e: KeyboardEvent) => this.onFieldKeydown(e)}
                style="background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 7px 9px; font-size: 0.9286rem; color: var(--fg-default); font-family: inherit;"
              />
            </label>
            <label style="display: flex; flex-direction: column; gap: 4px; font-size: 0.8571rem; color: var(--fg-muted);">
              Replace with
              <input
                id="fr-replace"
                type="text"
                .value=${this.replace}
                placeholder="Replacement text (blank deletes the match)"
                aria-label="Replacement text"
                @input=${(e: Event) => { this.replace = (e.target as HTMLInputElement).value; this.onTermChanged(); }}
                @keydown=${(e: KeyboardEvent) => this.onFieldKeydown(e)}
                style="background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 7px 9px; font-size: 0.9286rem; color: var(--fg-default); font-family: inherit;"
              />
            </label>

            <label class="modal-checkbox-row" style="margin: 0;">
              <input
                type="checkbox"
                class="toggle-switch"
                .checked=${this.caseSensitive}
                @change=${(e: Event) => { this.caseSensitive = (e.target as HTMLInputElement).checked; this.onTermChanged(); }}
              />
              <span class="modal-checkbox-label">Match case</span>
            </label>
          </div>

          <div style="display: flex; flex-direction: column; gap: 6px; margin-bottom: 14px;">
            <span style="font-size: 0.8571rem; color: var(--fg-muted);">Scope</span>
            <div role="radiogroup" aria-label="Scope" style="display: flex; gap: 8px;">
              ${this.hasRecording
                ? html`
                    <button
                      type="button"
                      role="radio"
                      aria-checked=${this.scope === "recording"}
                      class="fr-scope-btn ${this.scope === "recording" ? "active" : ""}"
                      @click=${() => this.setScope("recording")}
                      style=${this.scopeBtnStyle(this.scope === "recording")}
                    >This recording</button>
                  `
                : ""}
              <button
                type="button"
                role="radio"
                aria-checked=${this.scope === "library"}
                class="fr-scope-btn ${this.scope === "library" ? "active" : ""}"
                @click=${() => this.setScope("library")}
                style=${this.scopeBtnStyle(this.scope === "library")}
              >Whole library</button>
            </div>
          </div>

          ${this.scope === "library"
            ? html`
                <div
                  role="alert"
                  style="margin-bottom: 14px; padding: 10px 12px; border-radius: 6px; font-size: 0.8214rem; line-height: 1.5; color: var(--warn); background: color-mix(in srgb, var(--warn) 14%, transparent); border: 1px solid color-mix(in srgb, var(--warn) 40%, transparent);"
                >
                  <strong>Library-wide replace.</strong> This rewrites the live transcript of
                  <strong>every</strong> recording that matches and re-embeds each one for search.
                  The original/clean baselines are kept, so each edit is revertible — but there is no
                  single undo for the whole batch. Recordings with no match are left untouched.
                </div>
              `
            : ""}

          ${this.result
            ? html`<div
                role="status"
                aria-live="polite"
                style=${`margin-bottom: 14px; font-size: 0.8571rem; line-height: 1.4; color: ${this.resultKind === "ok" ? "var(--ok)" : "var(--err)"};`}
              >${this.result}</div>`
            : ""}

          <div class="modal-actions">
            <button class="modal-btn" @click=${() => this.close()}>Close</button>
            <button
              class="modal-btn ${runDanger ? "modal-btn-danger" : "modal-btn-primary"}"
              ?disabled=${this.busy}
              @click=${() => this.run()}
            >${runLabel}</button>
          </div>
        </div>
      </div>
    `;
  }

  private scopeBtnStyle(active: boolean): string {
    return [
      "flex: 1",
      "padding: 7px 10px",
      "border-radius: 6px",
      "font-size: 0.8571rem",
      "cursor: pointer",
      "transition: all var(--ui-motion) ease-out",
      `border: 1px solid ${active ? "var(--accent)" : "var(--border-subtle)"}`,
      `background: ${active ? "color-mix(in srgb, var(--accent) 16%, transparent)" : "var(--bg-surface)"}`,
      `color: ${active ? "var(--accent)" : "var(--fg-default)"}`,
    ].join("; ");
  }
}

/**
 * Open the Find / Replace modal; resolves when it closes. Pass a `recordingId`
 * to offer the "This recording" scope (the default when present); omit it for a
 * library-only run. Edits arrive back at the UI via the `TranscriptUpdated`
 * daemon events, so nothing needs the result.
 */
export function openFindReplace(recordingId?: string): Promise<void> {
  return new Promise((resolve) => {
    const el = document.createElement("ph-find-replace") as FindReplaceElement;
    if (recordingId) el.recordingId = recordingId;
    el.addEventListener("resolved", () => {
      closeModalHost(el, () => {
        el.remove();
        resolve();
      });
    });
    document.body.appendChild(el);
  });
}
