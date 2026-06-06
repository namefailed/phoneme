import { LitElement, html, css, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { listSession, type Recording } from "../../services/ipc";
import './RecordingDetail'; // Reusing the detail component for each track

@customElement('ph-merged-conversation-detail')
export class MergedConversationDetail extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for inherited CSS classes
  }

  @property({ type: String }) sessionId = "";
  @property({ type: Object }) onRefresh!: () => void;

  @state() private recordings: Recording[] = [];
  @state() private error: string | null = null;

  async updated(changedProperties: PropertyValues) {
    if (changedProperties.has('sessionId')) {
      if (this.sessionId) {
        await this.loadSession();
      } else {
        this.recordings = [];
      }
    }
  }

  private async loadSession() {
    this.error = null;
    try {
      this.recordings = await listSession(this.sessionId);
    } catch (e) {
      this.error = String(e);
      this.recordings = [];
    }
  }

  render() {
    if (this.error) {
      return html`<div class="error">Error loading meeting session: ${this.error}</div>`;
    }

    if (this.recordings.length === 0) {
      return html`<div class="empty">Loading meeting session...</div>`;
    }

    // Since we don't have word-level timestamps in the DB yet,
    // we display the tracks grouped side-by-side or stacked.
    return html`
      <div style="display: flex; flex-direction: column; gap: 1rem; padding: 1rem;">
        <div style="display: flex; justify-content: space-between; align-items: center; padding-bottom: 0.5rem; border-bottom: 1px solid var(--border-color, #ccc);">
          <h2 style="margin: 0; color: var(--text-color, #333);">Meeting Session: ${this.sessionId}</h2>
        </div>
        ${this.recordings.map(
          (rec) => html`
            <div style="display: flex; flex-direction: column; gap: 0.5rem; border: 1px solid var(--border-color, #ccc); border-radius: 8px; padding: 1rem; background: var(--bg-card, #f9f9f9);">
              <h3 style="margin: 0; color: var(--text-color, #333); text-transform: capitalize;">Track: ${rec.track || 'Unknown'}</h3>
              <ph-recording-detail
                .recordingId=${rec.id}
                .onRefresh=${this.onRefresh}
              ></ph-recording-detail>
            </div>
          `
        )}
      </div>
    `;
  }
}
