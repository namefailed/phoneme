import { errText } from "../../utils/error";
import { LitElement, html, nothing, PropertyValues } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { listSession, setSpeakerName, getSegments, type Recording, type TranscriptSegment } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { formatDuration, fmtClock } from "../../utils/format";
import {
  mergeMeeting,
  mergedPlainText,
  mergeChronological,
  chronoPlainText,
  type MergedBlock,
  type ChronoBlock,
} from "./mergeMeeting";
import { applyMoreLikeThis } from "../../state/filter";

/** Distinct, theme-agnostic colors so each speaker is easy to follow at a glance.
 *  Indexed by the 1-based speaker label; wraps for meetings with many speakers. */
const SPEAKER_COLORS = [
  "#89b4fa", "#a6e3a1", "#f9e2af", "#f38ba8",
  "#cba6f7", "#fab387", "#94e2d5", "#f5c2e7",
];
function speakerColor(label: number): string {
  return SPEAKER_COLORS[(Math.max(1, label) - 1) % SPEAKER_COLORS.length];
}
/** Short avatar text: the speaker number for default "Speaker N" labels, else
 *  the first letter of a custom name. */
function avatarText(displayName: string | null, label: number): string {
  const name = (displayName ?? "").trim();
  if (!name || name === `Speaker ${label}`) return String(label);
  return name.charAt(0).toUpperCase();
}

/**
 * The merged meeting view: a single, unified reading of every track in a
 * meeting, rendered in the right pane when the meeting's group header is
 * selected (the list emits `session:<meeting_id>` → index.ts sets `meetingId`).
 *
 * When every transcribed track carries persisted segment timing, the view is
 * a CHRONOLOGICAL chat timeline (mic turns left, the meeting's right, each
 * stamped with its clock offset — the tracks share a wall clock at capture).
 * Meetings transcribed before segment capture fall back to the coarse
 * by-source merge: tracks ordered by start time, each a labelled section with
 * the pipeline's embedded `[Speaker N]:` turns surfaced inside. Read-only
 * either way; clicking an individual track row still opens the editable
 * single-recording detail, and the Dual-timeline button explodes the meeting
 * into the synced split view.
 */
@customElement("ph-merged-conversation-detail")
export class MergedConversationDetail extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM so the shared theme/CSS classes apply.
  }

  @property({ type: String }) meetingId = "";
  @property({ type: Object }) onRefresh!: () => void;

  @state() private recordings: Recording[] = [];
  /** Persisted segment timelines per track id — drives the chronological
   *  (chat-style) merge. A track with no stored segments maps to []. */
  @state() private segmentsMap: Map<string, TranscriptSegment[]> = new Map();
  @state() private error: string | null = null;
  @state() private loading = false;
  @state() private copyLabel = "📋 Copy";
  /** The speaker chip currently being renamed (which track + which 1-based
   *  label), or null when none. Click a chip to edit; commit on Enter/blur. */
  @state() private editing: { recordingId: string; label: number } | null = null;

  async updated(changedProperties: PropertyValues) {
    if (changedProperties.has("meetingId")) {
      if (this.meetingId) {
        await this.loadSession();
      } else {
        this.recordings = [];
        this.error = null;
      }
    }
    // When a speaker chip enters edit mode, focus + select its input so the
    // user can type the name immediately.
    if (changedProperties.has("editing") && this.editing) {
      const input = this.querySelector<HTMLInputElement>(".merged-speaker-input");
      if (input) {
        input.focus();
        input.select();
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
      // Segment timelines make the merge chronological; a track without them
      // (transcribed before segment capture) just falls back to the coarse
      // by-source merge, so fetch failures degrade silently to [].
      const entries = await Promise.all(
        this.recordings.map(async (r) => {
          const segs = await getSegments(r.id).catch(() => [] as TranscriptSegment[]);
          return [r.id, segs] as const;
        }),
      );
      this.segmentsMap = new Map(entries);
    } catch (e) {
      this.error = errText(e);
      this.recordings = [];
      this.segmentsMap = new Map();
    } finally {
      this.loading = false;
    }
  }

  private get blocks(): MergedBlock[] {
    return mergeMeeting(this.recordings);
  }

  /** The time-interleaved reading, or null when any track lacks segment
   *  timing (then the coarse by-source merge renders instead). */
  private get chronoBlocks(): ChronoBlock[] | null {
    return mergeChronological(this.recordings, this.segmentsMap);
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

  /** Commit a speaker rename for `(recordingId, label)`. An empty value clears
   *  the custom name (reverts to "Speaker N"). Persists via IPC, then reloads
   *  the tracks so every occurrence of that speaker re-renders with the name. */
  private async commitSpeakerName(recordingId: string, label: number, value: string) {
    this.editing = null;
    try {
      await setSpeakerName(recordingId, label, value.trim());
      // The merged view renders names from the speaker-names map (mergeMeeting)
      // and splits turns on the [Speaker N] markers — so DON'T bake names into
      // the track transcript here; that would destroy the per-speaker structure.
      await this.loadSession();
      this.onRefresh?.();
    } catch (e) {
      showToast(`Couldn't rename speaker: ${errText(e)}`, "error");
    }
  }

  private onSpeakerInputKeyDown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      (e.target as HTMLInputElement).blur();
    } else if (e.key === "Escape") {
      e.preventDefault();
      this.editing = null; // cancel without saving
    }
  }

  /** Explode this meeting into the dual-timeline split: both tracks side by
   *  side as synced, clickable timelines (a click seeks both waveforms; the
   *  tracks are wall-clock synced at capture, so equal offsets line up).
   *  Closing the split (Esc/✕) returns to this merged view. */
  private openDualTimeline() {
    const ordered = [...this.recordings].sort((a, b) =>
      (a.track ?? "").localeCompare(b.track ?? ""),
    ); // "mic" before "system"
    const [a, b] = ordered;
    if (!a || !b) return;
    window.dispatchEvent(
      new CustomEvent("phoneme:open-split", {
        detail: { a: a.id, b: b.id, timeline: true, returnTo: `session:${this.meetingId}` },
      }),
    );
  }

  private async handleCopy() {
    try {
      const chrono = this.chronoBlocks;
      await navigator.clipboard.writeText(
        chrono ? chronoPlainText(chrono) : mergedPlainText(this.blocks),
      );
      this.copyLabel = "✅ Copied!";
      setTimeout(() => {
        this.copyLabel = "📋 Copy";
      }, 2000);
    } catch (e) {
      showToast(`Clipboard copy failed: ${errText(e)}`, "error");
    }
  }

  /** "More like this" for the whole meeting: seed the similarity search from
   *  one transcribed track. Track choice barely matters — the daemon excludes
   *  the meeting by its dedupe key (so neither of THIS meeting's tracks comes
   *  back) and the tracks' transcripts are near-identical — so take the first
   *  track that has a transcript. The pill is labelled with the meeting name. */
  private handleMoreLikeThis() {
    const seed = this.recordings.find((r) => (r.transcript ?? "").trim()) ?? this.recordings[0];
    if (!seed) return;
    applyMoreLikeThis(seed.id, this.recordings[0]?.meeting_name || null);
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
        const chrono = this.chronoBlocks;
        await writeTextFile(dest, chrono ? chronoPlainText(chrono) : mergedPlainText(this.blocks));
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

    const chrono = this.chronoBlocks;
    const blocks: MergedBlock[] = chrono ?? this.blocks;
    const meetingName = this.recordings[0]?.meeting_name || this.meetingId;
    // Both tracks of a meeting share a start time, so any track's is fine.
    const totalDuration = this.recordings.reduce(
      (max, r) => Math.max(max, r.duration_ms ?? 0),
      0,
    );
    const sourceCount = new Set(this.recordings.map((r) => r.track ?? "")).size;
    const speakerCount = new Set(blocks.filter((b) => b.speaker != null).map((b) => b.speaker)).size;
    const turnCount = blocks.length;

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
              ${this.recordings.length >= 2
                ? html`<button
                    class="inline-button"
                    title="Open both tracks side by side as synced timelines — click a line to seek both"
                    @click=${this.openDualTimeline}
                  >🕒 Dual timeline</button>`
                : nothing}
              <button class="inline-button" @click=${this.handleCopy}>${this.copyLabel}</button>
              <button class="inline-button" @click=${this.handleExport}>⬇ Export</button>
              <button class="inline-button" title="More like this — fill the list with recordings about similar things, found from this meeting's semantic index" @click=${this.handleMoreLikeThis}>✨ Similar</button>
            </div>
          </div>
          <div class="merged-meta">
            <span class="merged-meta-pill">${sourceCount} ${sourceCount === 1 ? "track" : "tracks"}</span>
            <span class="merged-meta-pill">${formatDuration(totalDuration)}</span>
            ${speakerCount > 0 ? html`<span class="merged-meta-pill">${speakerCount} ${speakerCount === 1 ? "speaker" : "speakers"}</span>` : nothing}
            <span class="merged-meta-pill">${turnCount} ${turnCount === 1 ? "turn" : "turns"}</span>
            ${chrono
              ? html`<span class="merged-meta-pill merged-meta-pill--chrono" title="Turns are interleaved by their real timestamps — the tracks share a wall clock at capture">🕒 chronological</span>`
              : nothing}
            <span class="merged-meta-ro">merged reading · read-only</span>
          </div>
        </div>

        ${blocks.length === 0
          ? html`<div class="empty">No transcript yet for this meeting.</div>`
          : chrono
            ? html`<div class="merged-body chrono-body">
                ${chrono.map((b) => this.renderChronoBlock(b))}
              </div>`
            : html`<div class="merged-body">
                ${blocks.map((b, i) => this.renderBlock(b, blocks[i - 1]))}
              </div>`}
      </div>
    `;
  }

  /** Render one CHRONOLOGICAL turn as a chat row: mic ("you") on the left,
   *  everything else (the meeting) on the right, stamped with its clock
   *  offset. Numeric speakers keep the renamable chip; cloud letter labels
   *  render as static text. */
  private renderChronoBlock(b: ChronoBlock) {
    const isMic = b.source.track === "mic";
    const color = b.speaker != null ? speakerColor(b.speaker) : "var(--fg-faded)";
    return html`
      <div class="chrono-row ${isMic ? "" : "chrono-row--right"}" data-track=${b.source.track}>
        <div class="chrono-bubble" style=${`--spk:${color}`}>
          <div class="chrono-head">
            <span class="chrono-time">${fmtClock(b.startMs)}</span>
            <span class="chrono-source" aria-hidden="true" title=${b.source.label}>${b.source.icon}</span>
            ${b.speaker != null
              ? this.renderSpeakerChip(b)
              : b.displayName
                ? html`<span class="merged-speaker">${b.displayName}</span>`
                : nothing}
          </div>
          <div class="chrono-text">${b.text}</div>
        </div>
      </div>
    `;
  }

  /** Render one merged block. The source header is repeated only when the source
   *  changes from the previous block, so a run of same-source turns reads as one
   *  contiguous section. */
  private renderBlock(b: MergedBlock, prev: MergedBlock | undefined) {
    const newSource = !prev || prev.source.track !== b.source.track;
    const hasSpeaker = b.speaker != null;
    const color = hasSpeaker ? speakerColor(b.speaker as number) : "var(--fg-faded)";
    return html`
      ${newSource
        ? html`<div class="merged-source" data-track=${b.source.track}>
            <span class="merged-source-icon" aria-hidden="true">${b.source.icon}</span>
            <span class="merged-source-label">${b.source.label}</span>
          </div>`
        : nothing}
      <div class="merged-turn ${hasSpeaker ? "" : "merged-turn--prose"}" style=${`--spk:${color}`}>
        ${hasSpeaker
          ? html`<div class="merged-avatar" aria-hidden="true">${avatarText(b.displayName, b.speaker as number)}</div>`
          : nothing}
        <div class="merged-turn-body">
          ${hasSpeaker ? html`<div class="merged-turn-head">${this.renderSpeakerChip(b)}</div>` : nothing}
          <div class="merged-text">${b.text}</div>
        </div>
      </div>
    `;
  }

  /** The clickable speaker label for a turn. Shows the custom name (or
   *  "Speaker N"); clicking swaps it for an inline text input that persists the
   *  rename on Enter/blur. Renaming applies to every turn of that speaker in the
   *  track because the stored mapping — not the per-block text — is the source
   *  of truth. */
  private renderSpeakerChip(b: MergedBlock) {
    const label = b.speaker as number;
    const isEditing =
      this.editing?.recordingId === b.recordingId && this.editing?.label === label;
    if (isEditing) {
      // Start from the custom name if one is set, else blank so the user types
      // a fresh name rather than editing the "Speaker N" placeholder.
      const current = b.displayName === `Speaker ${label}` ? "" : (b.displayName ?? "");
      return html`<input
        class="merged-speaker merged-speaker-input"
        .value=${current}
        placeholder=${`Speaker ${label}`}
        spellcheck="false"
        aria-label=${`Rename Speaker ${label}`}
        @keydown=${this.onSpeakerInputKeyDown}
        @blur=${(e: Event) =>
          this.commitSpeakerName(b.recordingId, label, (e.target as HTMLInputElement).value)}
      />`;
    }
    return html`<button
      class="merged-speaker merged-speaker-button"
      type="button"
      title="Click to rename this speaker"
      @click=${() => (this.editing = { recordingId: b.recordingId, label })}
    >
      ${b.displayName ?? `Speaker ${label}`}
    </button>`;
  }
}
