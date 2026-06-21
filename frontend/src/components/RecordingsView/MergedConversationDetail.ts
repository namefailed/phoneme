import { errText } from "../../utils/error";
import { LitElement, html, nothing, PropertyValues } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { listSession, setSpeakerName, getSegments, saveTextExport, recognizeSpeakers, dismissSpeakerSuggestion, getMeetingDigest, rerunMeetingDigest, type Recording, type TranscriptSegment, type SpeakerSuggestion, type MeetingDigest } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { formatDuration, fmtClock } from "../../utils/format";
import { invoke } from "@tauri-apps/api/core";
import {
  mergeMeeting,
  mergedPlainText,
  mergeChronological,
  chronoPlainText,
  type MergedBlock,
  type ChronoBlock,
} from "./mergeMeeting";
import { applyMoreLikeThis } from "../../state/filter";
import { subscribe, type DaemonEvent } from "../../services/events";
import { applyLlmActivity, emptyLlmStream, matchesLlmStream, type LlmStreamState } from "./llmStream";

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

  /** Recognized-speaker suggestions across the meeting's tracks (#9): unnamed
   *  diarized speakers whose voiceprints matched a known voice. Each carries the
   *  track `recordingId` so accepting names the right track's speaker. */
  @state() private suggestions: (SpeakerSuggestion & { recordingId: string })[] = [];
  /** App config, for the `interface.format_24h` time-of-day preference shown on
   *  chronological turns. Loaded once and refreshed on `config:saved` (same as
   *  the recordings list). */
  @state() private config: any = null;

  /** The stored whole-meeting digest (LLM synthesis across every track), or null
   *  when none has been generated yet. The meeting-scope twin of a recording's
   *  `summary`; fetched alongside the tracks. */
  @state() private digest: MeetingDigest | null = null;
  /** Whether a digest (re)generation is in flight, so the card shows a pending
   *  state and the button is disabled. Cleared by the `meeting_digest_*` daemon
   *  event (the parent calls `reload()`), or on a request error. */
  @state() private digestPending = false;

  /** Live accumulation of the streamed digest (the `summarizing` LLM stage keyed
   *  on the meeting's first track) so the card shows tokens as they generate,
   *  the same `llm_activity` stream the AI-activity popout consumes. Display-only:
   *  the daemon caps it, and the `meeting_digest_updated` reload settles the card
   *  to the full stored `digest`. Only painted while `digestPending` (Layer 2.4):
   *  a per-track summary can stream on this same id + stage, so the pending flag
   *  is what tells the digest's stream apart from a stray per-track one. */
  @state() private digestStream: LlmStreamState = emptyLlmStream();
  /** The meeting a pending digest stream belongs to, so a reload that switches to
   *  a different meeting can drop the stale stream (a same-meeting reload keeps
   *  it). Null when no digest is in flight. */
  private digestMeetingId: string | null = null;

  private onConfigSaved = (e: Event) => {
    this.config = (e as CustomEvent).detail ?? null;
  };

  /** Teardown for the daemon-event subscription that feeds `digestStream`. */
  private llmUnsub: (() => void) | null = null;
  /** Set once the element disconnected, so a subscription that resolves after
   *  teardown unlistens itself instead of leaking. */
  private gone = false;

  connectedCallback() {
    super.connectedCallback();
    window.addEventListener("config:saved", this.onConfigSaved);
    if (!this.config) {
      invoke("read_config").then((cfg) => {
        this.config = cfg;
      }).catch(console.error);
    }
    this.gone = false;
    void subscribe((event: DaemonEvent) => this.onLlmActivity(event)).then((unsub) => {
      if (this.gone) unsub();
      else this.llmUnsub = unsub;
    });
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("config:saved", this.onConfigSaved);
    this.gone = true;
    if (this.llmUnsub) {
      this.llmUnsub();
      this.llmUnsub = null;
    }
  }

  /** Consume the digest's `summarizing` LLM stream (keyed on the meeting's first
   *  track, where the daemon emits the digest's activity — `generate_meeting_digest`)
   *  and accumulate it into `digestStream`. Only matters while `digestPending`,
   *  so a per-track summary streaming on the same id+stage can't cross-paint the
   *  digest card (Layer 2.4 / risk 3). The `@state` write re-renders the card. */
  private onLlmActivity(event: DaemonEvent) {
    if (!this.digestPending) return;
    const firstTrackId = this.recordings[0]?.id;
    if (!firstTrackId || !matchesLlmStream(event, { id: firstTrackId, stage: "summarizing" })) return;
    if (event.event !== "llm_activity") return; // narrow for applyLlmActivity
    this.digestStream = applyLlmActivity(this.digestStream, event);
  }

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
    this.suggestions = [];
    // A reload for a DIFFERENT meeting drops any in-flight digest stream/pending
    // from the previous one, so a late delta for the old meeting's digest can't
    // paint into the one now loading. A reload for the SAME meeting (e.g. a track
    // finished transcribing while a digest is mid-stream) leaves the live stream
    // untouched — the digest's own result lands via the dedicated meeting_digest
    // reload path, which repopulates `digest` and clears pending below.
    if (this.digestMeetingId && this.digestMeetingId !== this.meetingId) {
      this.digestPending = false;
      this.digestStream = emptyLlmStream();
      this.digestMeetingId = null;
    }
    // Capture the meeting this load is for; each await below can resolve after
    // the user has switched to a different meeting, and assigning a previous
    // meeting's tracks/segments/digest onto the current one paints the wrong
    // reading (audit — mid-flight meeting switch). Bail before EACH assignment
    // if we've moved on, mirroring loadSuggestions()'s guard.
    const mid = this.meetingId;
    try {
      const recordings = await listSession(mid);
      if (this.meetingId !== mid) return;
      this.recordings = recordings;
      // Segment timelines make the merge chronological; a track without them
      // (transcribed before segment capture) just falls back to the coarse
      // by-source merge, so fetch failures degrade silently to [].
      const entries = await Promise.all(
        recordings.map(async (r) => {
          const segs = await getSegments(r.id).catch(() => [] as TranscriptSegment[]);
          return [r.id, segs] as const;
        }),
      );
      if (this.meetingId !== mid) return;
      this.segmentsMap = new Map(entries);
      // The whole-meeting digest is fetched alongside the tracks; a missing
      // digest is the normal "not generated yet" state (null), not an error.
      const digest = await getMeetingDigest(mid).catch(() => null);
      if (this.meetingId !== mid) return;
      this.digest = digest;
      // A finished (re)generation clears the pending state when its result lands.
      // But an unrelated reload (e.g. a track finished transcribing) while a
      // digest is still streaming must NOT clear pending — that would freeze the
      // live stream. Keep pending only while the stream is still arriving and no
      // newer digest landed; the dedicated meeting_digest reload brings the
      // stored digest, which clears it here.
      if (digest || !this.digestStream.streaming) {
        this.digestPending = false;
        this.digestStream = emptyLlmStream();
        this.digestMeetingId = null;
      }
    } catch (e) {
      if (this.meetingId === mid) {
        this.error = errText(e);
        this.recordings = [];
        this.segmentsMap = new Map();
        this.digest = null;
      }
    } finally {
      this.loading = false;
    }
    // Recognition is a non-blocking convenience — let the reading render first,
    // then fill the banner when the matches come back. Skip if we've switched
    // meetings; the new meeting's load runs its own loadSuggestions().
    if (this.meetingId === mid) void this.loadSuggestions();
  }

  /** Recognize unnamed speakers across every track and collect the suggestions
   *  (the daemon already skips named/dismissed labels, and returns nothing when
   *  recognition is off or a track was cloud-diarized). */
  private async loadSuggestions() {
    // Capture the meeting this batch is for; a slower recognition batch from a
    // PREVIOUS meeting must not paint its (wrong-track) suggestions onto the one
    // now shown (audit — wrong-track naming). Bail before assigning if we've moved.
    const mid = this.meetingId;
    const recs = this.recordings;
    if (recs.length === 0) {
      if (this.meetingId === mid) this.suggestions = [];
      return;
    }
    try {
      const perTrack = await Promise.all(
        recs.map((r) =>
          recognizeSpeakers(r.id)
            .then((list) => list.map((s) => ({ ...s, recordingId: r.id })))
            .catch(() => [] as (SpeakerSuggestion & { recordingId: string })[]),
        ),
      );
      if (this.meetingId !== mid) return;
      this.suggestions = perTrack.flat();
    } catch {
      if (this.meetingId === mid) this.suggestions = [];
    }
  }

  /** Accept a suggestion: name that track's speaker (which enrolls + reinforces
   *  the voice). `commitSpeakerName` reloads the session, refreshing the banner. */
  private acceptSuggestion(s: SpeakerSuggestion & { recordingId: string }) {
    void this.commitSpeakerName(s.recordingId, s.speaker_label, s.name);
  }

  /** Dismiss a suggestion so it isn't offered again for that track + speaker. */
  private async dismissSuggestion(s: SpeakerSuggestion & { recordingId: string }) {
    this.suggestions = this.suggestions.filter(
      (x) => !(x.recordingId === s.recordingId && x.speaker_label === s.speaker_label),
    );
    try {
      await dismissSpeakerSuggestion(s.recordingId, s.speaker_label);
    } catch {
      /* dismissal is best-effort */
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
   *  the meeting by its dedupe key (so neither of this meeting's tracks comes
   *  back) and the tracks' transcripts are near-identical — so take the first
   *  track that has a transcript. The pill is labelled with the meeting name. */
  private handleMoreLikeThis() {
    const seed = this.recordings.find((r) => (r.transcript ?? "").trim()) ?? this.recordings[0];
    if (!seed) return;
    applyMoreLikeThis(seed.id, this.recordings[0]?.meeting_name || null);
  }

  /** Generate (or regenerate) the whole-meeting digest. The daemon ACKs
   *  immediately and runs the LLM in the background; the result lands via the
   *  `meeting_digest_updated` event, which the parent turns into a `reload()` —
   *  so we only flip the pending state here and let the reload paint the result. */
  private async handleGenerateDigest() {
    if (this.digestPending || !this.meetingId) return;
    // Drop any prior live buffer so this run's stream starts clean (the daemon's
    // prompt-start resets it too); flip pending so onLlmActivity starts painting.
    this.digestStream = emptyLlmStream();
    this.digestMeetingId = this.meetingId;
    this.digestPending = true;
    try {
      await rerunMeetingDigest(this.meetingId);
    } catch (e) {
      this.digestPending = false;
      showToast(`Couldn't generate meeting digest: ${errText(e)}`, "error");
    }
  }

  private async handleExport() {
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const meetingName = this.recordings[0]?.meeting_name || this.meetingId;
      const safeName = meetingName.replace(/[^\w.-]+/g, "_");
      const dest = await save({
        defaultPath: `meeting-${safeName}.txt`,
        filters: [{ name: "Text", extensions: ["txt"] }],
      });
      if (dest) {
        const chrono = this.chronoBlocks;
        // Write server-side (the WebView can't write an arbitrary save-dialog
        // path via the fs plugin — see saveTextExport).
        await saveTextExport(dest, chrono ? chronoPlainText(chrono) : mergedPlainText(this.blocks));
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
              <button class="inline-button" style="display:inline-flex; align-items:center; gap:5px;" title="More like this — fill the list with recordings about similar things, found from this meeting's semantic index" @click=${this.handleMoreLikeThis}><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>Similar</button>
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

        ${this.suggestions.length
          ? html`<div class="merged-suggest">
              <span class="merged-suggest-lead">🔎 Recognized voices:</span>
              ${this.suggestions.map(
                (s) => html`<span class="merged-suggest-chip">
                  Speaker ${s.speaker_label} sounds like
                  <b>${s.name}</b>
                  <span class="merged-suggest-pct">${Math.round((s.score ?? 0) * 100)}%</span>
                  <button
                    class="merged-suggest-yes"
                    title="Use this name"
                    @click=${() => this.acceptSuggestion(s)}
                  >Use</button>
                  <button
                    class="merged-suggest-no"
                    title="Not them — don't suggest again"
                    @click=${() => this.dismissSuggestion(s)}
                  >✗</button>
                </span>`,
              )}
            </div>`
          : nothing}

        ${this.renderDigest(blocks.length > 0)}

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

  /** Render the whole-meeting digest card — the LLM synthesis across every
   *  track, the meeting-scope twin of a recording's summary peek. Shows the
   *  stored digest (with the model that produced it) plus a Regenerate button;
   *  when none exists yet, a "Generate digest" affordance. `haveTranscript` gates
   *  the generate button so it isn't offered before any track has transcribed. */
  private renderDigest(haveTranscript: boolean) {
    const d = this.digest;
    const modelNote = d?.digest_model
      ? html` · <span style="opacity:0.8">${d.digest_model}</span>`
      : nothing;
    const btnLabel = this.digestPending
      ? "Generating…"
      : d
        ? "↻ Regenerate"
        : "✨ Generate digest";
    // Nothing to generate from yet: no transcript and no stored digest → hide
    // the card entirely (matches "No transcript yet for this meeting" below).
    if (!d && !haveTranscript && !this.digestPending) return nothing;
    return html`
      <div class="merged-digest">
        <div class="merged-digest-head">
          <span class="merged-digest-label">✨ Meeting digest${modelNote}</span>
          <button
            class="inline-button"
            ?disabled=${this.digestPending || !haveTranscript}
            title="Generate one AI digest synthesizing every track of this meeting"
            @click=${this.handleGenerateDigest}
          >${btnLabel}</button>
        </div>
        ${this.digestPending && !d
          ? this.digestStream.text
            // Stream the digest live, token by token, with a spinner while more
            // is still arriving. White-space pre-wrap matches the settled digest.
            ? html`<div class="merged-digest-body" style="white-space: pre-wrap">${this.digestStream.text}${this.digestStream.streaming ? html`<span class="thinking-spin" aria-hidden="true" style="margin-left: 6px; vertical-align: middle"></span>` : nothing}</div>`
            : html`<div class="merged-digest-body" style="color: var(--fg-muted)">Generating the meeting digest…</div>`
          : d
            ? html`<div class="merged-digest-body">${d.digest}</div>`
            : html`<div class="merged-digest-body" style="color: var(--fg-muted)">No digest yet — generate one cohesive summary across both tracks.</div>`}
      </div>
    `;
  }

  /** The real wall-clock time-of-day of a turn, as `HH:MM:SS`. The tracks
   *  share a wall clock at capture, so the turn happened at the meeting's
   *  `started_at` plus the turn's file offset. Honours the
   *  `interface.format_24h` preference, the same way the recordings list
   *  renders its time-of-day column (see `formatTime`). Returns "" when the
   *  meeting start is unknown. */
  private clockTimeOfDay(offsetMs: number): string {
    const startIso = this.recordings[0]?.started_at;
    if (!startIso) return "";
    const d = new Date(new Date(startIso).getTime() + offsetMs);
    const use24h = this.config?.interface?.format_24h ?? false;
    return d.toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hour12: !use24h,
    });
  }

  /** Render one CHRONOLOGICAL turn as a chat row: mic ("you") on the left,
   *  everything else (the meeting) on the right, stamped with both the real
   *  wall-clock time-of-day and the file offset (`10:05:13 · 0:13`). Numeric
   *  speakers keep the renamable chip; cloud letter labels render as static
   *  text. */
  private renderChronoBlock(b: ChronoBlock) {
    const isMic = b.source.track === "mic";
    const color = b.speaker != null ? speakerColor(b.speaker) : "var(--fg-faded)";
    const clock = this.clockTimeOfDay(b.startMs);
    return html`
      <div class="chrono-row ${isMic ? "" : "chrono-row--right"}" data-track=${b.source.track}>
        <div class="chrono-bubble" style=${`--spk:${color}`}>
          <div class="chrono-head">
            <span class="chrono-time"
              >${clock
                ? html`<span title="Time of day">${clock}</span> <span style="opacity:0.6" title="Offset from the start of the recording">· ${fmtClock(b.startMs)}</span>`
                : fmtClock(b.startMs)}</span
            >
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
