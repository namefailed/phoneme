import { errText } from "../../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { listSession, type Recording } from "../../services/ipc";
import { RecordingDetail } from "./RecordingDetail";

@customElement('ph-merged-conversation-detail')
export class MergedConversationDetail extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for inherited CSS classes
  }

  @property({ type: String }) meetingId = "";
  @property({ type: Object }) onRefresh!: () => void;

  @state() private recordings: Recording[] = [];
  @state() private error: string | null = null;

  /** One RecordingDetail per track, mounted imperatively into the rendered
   *  containers. Kept so we can tear down WaveSurfer/CodeMirror when the
   *  session changes or the element is removed. */
  private details: RecordingDetail[] = [];
  /** Identity of the track set the mounted `details` represent. We only remount
   *  when this changes, so unrelated re-renders don't recreate the waveforms. */
  private mountedKey = "";

  async updated(changedProperties: PropertyValues) {
    if (changedProperties.has('meetingId')) {
      if (this.meetingId) {
        await this.loadSession();
      } else {
        this.recordings = [];
        this.error = null;
      }
    }
    this.syncDetails();
  }

  private async loadSession() {
    this.error = null;
    try {
      this.recordings = await listSession(this.meetingId);
    } catch (e) {
      this.error = errText(e);
      this.recordings = [];
    }
  }

  /** Mount one RecordingDetail per track into the rendered `.merged-track-detail`
   *  containers, but only when the track set actually changed. */
  private syncDetails() {
    const key = this.recordings.map((r) => r.id).join(",");
    if (key === this.mountedKey) return;

    const roots = this.querySelectorAll<HTMLElement>(".merged-track-detail");
    if (roots.length !== this.recordings.length) return; // template not rendered yet

    this.disposeDetails();
    this.recordings.forEach((rec, i) => {
      const detail = new RecordingDetail(roots[i], () => this.onRefresh?.());
      void detail.show(rec.id);
      this.details.push(detail);
    });
    this.mountedKey = key;
  }

  private disposeDetails() {
    for (const d of this.details) d.clear();
    this.details = [];
    this.mountedKey = "";
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.disposeDetails();
  }

  private async saveMeetingName(newName: string) {
    if (!newName.trim()) return;
    try {
      const { updateMeetingName } = await import('../../services/ipc');
      await updateMeetingName(this.meetingId, newName.trim());
      await this.loadSession();
    } catch (e) {
      this.error = errText(e);
    }
  }

  private handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      (e.target as HTMLElement).blur();
    }
  }

  render() {
    if (this.error) {
      return html`<div class="error">Error loading meeting session: ${this.error}</div>`;
    }

    if (this.recordings.length === 0) {
      return html`<div class="empty">Loading meeting session…</div>`;
    }

    // Assuming both tracks have the same meeting_name, use the first one
    const meetingName = this.recordings[0]?.meeting_name || this.meetingId;

    return html`
      <div style="display: flex; flex-direction: column; gap: 1rem; padding: 1rem;">
        <div style="display: flex; justify-content: space-between; align-items: center; padding-bottom: 0.5rem; border-bottom: 1px solid var(--border-subtle);">
          <h2 style="margin: 0; color: var(--fg-default); display: flex; align-items: center; gap: 8px;">
            Meeting: 
            <span 
              contenteditable="true"
              style="padding: 2px 6px; border-radius: 4px; border: 1px solid transparent; outline: none; transition: border 0.2s;"
              @blur=${(e: Event) => this.saveMeetingName((e.target as HTMLElement).innerText)}
              @keydown=${this.handleKeyDown}
              @focus=${(e: Event) => (e.target as HTMLElement).style.border = '1px solid var(--accent)'}
              @focusout=${(e: Event) => (e.target as HTMLElement).style.border = '1px solid transparent'}
              title="Click to rename this meeting"
            >${meetingName}</span>
          </h2>
        </div>
        ${this.recordings.map(
          (rec) => html`
            <div style="display: flex; flex-direction: column; gap: 0.5rem; border: 1px solid var(--border-subtle); border-radius: 8px; padding: 1rem; background: var(--bg-surface);">
              <h3 style="margin: 0; color: var(--fg-default); text-transform: capitalize;">Track: ${rec.track || 'Unknown'}</h3>
              <div class="merged-track-detail"></div>
            </div>
          `
        )}
      </div>
    `;
  }
}
