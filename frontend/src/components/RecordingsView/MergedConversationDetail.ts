import { errText } from "../../utils/error";
import { LitElement, html, nothing, PropertyValues } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { listSession, type Recording } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { formatDuration } from "../../utils/format";
import { mergeMeeting, mergedPlainText, type MergedBlock } from "./mergeMeeting";

/**
 * The merged meeting view: a single, unified reading of every track in a
 * meeting, rendered in the right pane when the meeting's group header is
 * selected (the list emits `session:<meeting_id>` → index.ts sets `meetingId`).
 *
 * Per-segment timestamps aren't persisted, so this is a *coarse* merge — tracks
 * ordered by start time, each rendered as a labelled section, with the
 * pipeline's embedded `[Speaker N]:` turns surfaced inside. See
 * docs/design/merged-meeting-view.md for the full rationale and the follow-up
 * that would unlock true time-interleaving. The view is read-only; clicking an
 * individual track row still opens the editable single-recording detail.
 */
@customElement("ph-merged-conversation-detail")
export class MergedConversationDetail extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM so the shared theme/CSS classes apply.
  }

  @property({ type: String }) meetingId = "";
  @property({ type: Object }) onRefresh!: () => void;

  @state() private recordings: Recording[] = [];
  @state() private error: string | null = null;
  @state() private loading = false;
  @state() private copyLabel = "📋 Copy";

  async updated(changedProperties: PropertyValues) {
    if (changedProperties.has("meetingId")) {
      if (this.meetingId) {
        await this.loadSession();
      } else {
        this.recordings = [];
        this.error = null;
      }
    }
  }

  /** Re-fetch the meeting's tracks. Called by the parent on daemon events so the
   *  merged reading updates live when a track finishes transcribing — Lit won't
   *  re-run `updated` when `meetingId` is reassigned its current value. */
  async reload() {
    if (this.meetingId) await this.loadSession();
  }

  private async loadSession() {
    this.loading = true;
    this.error = null;
    try {
      this.recordings = await listSession(this.meetingId);
    } catch (e) {
      this.error = errText(e);
      this.recordings = [];
    } finally {
      this.loading = false;
    }
  }

  private get blocks(): MergedBlock[] {
    return mergeMeeting(this.recordings);
  }

  private async saveMeetingName(newName: string) {
    const trimmed = newName.trim();
    const current = this.recordings[0]?.meeting_name ?? "";
    if (trimmed === current) return;
    try {
      const { updateMeetingName } = await import("../../services/ipc");
      await updateMeetingName(this.meetingId, trimmed === "" ? null : trimmed);
      await this.loadSession();
      this.onRefresh?.();
    } catch (e) {
      this.error = errText(e);
    }
  }

  private handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      (e.target as HTMLElement).blur();
    }
  }

  private async handleCopy() {
    try {
      await navigator.clipboard.writeText(mergedPlainText(this.blocks));
      this.copyLabel = "✅ Copied!";
      setTimeout(() => {
        this.copyLabel = "📋 Copy";
      }, 2000);
    } catch (e) {
      showToast(`Clipboard copy failed: ${errText(e)}`, "error");
    }
  }

  private async handleExport() {
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const { writeTextFile } = await import("@tauri-apps/plugin-fs");
      const meetingName = this.recordings[0]?.meeting_name || this.meetingId;
      const safeName = meetingName.replace(/[^\w.-]+/g, "_");
      const dest = await save({
        defaultPath: `meeting-${safeName}.txt`,
        filters: [{ name: "Text", extensions: ["txt"] }],
      });
      if (dest) {
        await writeTextFile(dest, mergedPlainText(this.blocks));
        showToast("Merged transcript exported", "success");
      }
    } catch (e) {
      showToast(`Export failed: ${errText(e)}`, "error");
    }
  }

  render() {
    if (this.error) {
      return html`<div class="empty error">Couldn't load this meeting: ${this.error}</div>`;
    }
    if (this.loading && this.recordings.length === 0) {
      return html`<div class="empty">Loading meeting…</div>`;
    }
    if (this.recordings.length === 0) {
      return html`<div class="empty">No tracks found for this meeting.</div>`;
    }

    const blocks = this.blocks;
    const meetingName = this.recordings[0]?.meeting_name || this.meetingId;
    // Both tracks of a meeting share a start time, so any track's is fine.
    const totalDuration = this.recordings.reduce(
      (max, r) => Math.max(max, r.duration_ms ?? 0),
      0,
    );
    const sourceCount = new Set(this.recordings.map((r) => r.track ?? "")).size;

    return html`
      <div class="merged-detail">
        <div class="merged-header">
          <div class="merged-title-row">
            <h2 class="merged-title">
              <span aria-hidden="true">👥</span>
              <span
                class="merged-name"
                contenteditable="true"
                spellcheck="false"
                title="Click to rename this meeting"
                @blur=${(e: Event) => this.saveMeetingName((e.target as HTMLElement).innerText)}
                @keydown=${this.handleKeyDown}
                >${meetingName}</span
              >
            </h2>
            <div class="merged-actions">
              <button class="inline-button" @click=${this.handleCopy}>${this.copyLabel}</button>
              <button class="inline-button" @click=${this.handleExport}>⬇ Export</button>
            </div>
          </div>
          <div class="merged-meta">
            ${sourceCount} ${sourceCount === 1 ? "track" : "tracks"} ·
            ${formatDuration(totalDuration)} · merged reading (read-only)
          </div>
        </div>

        ${blocks.length === 0
          ? html`<div class="empty">No transcript yet for this meeting.</div>`
          : html`<div class="merged-body">
              ${blocks.map((b, i) => this.renderBlock(b, blocks[i - 1]))}
            </div>`}
      </div>
    `;
  }

  /** Render one merged block. The source header is repeated only when the source
   *  changes from the previous block, so a run of same-source turns reads as one
   *  contiguous section. */
  private renderBlock(b: MergedBlock, prev: MergedBlock | undefined) {
    const newSource = !prev || prev.source.track !== b.source.track;
    return html`
      ${newSource
        ? html`<div class="merged-source" data-track=${b.source.track}>
            <span class="merged-source-icon" aria-hidden="true">${b.source.icon}</span>
            <span class="merged-source-label">${b.source.label}</span>
          </div>`
        : nothing}
      <div class="merged-turn">
        ${b.speaker != null
          ? html`<span class="merged-speaker">Speaker ${b.speaker}</span>`
          : nothing}
        <span class="merged-text">${b.text}</span>
      </div>
    `;
  }
}
