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

  private async saveSessionName(newName: string) {
    if (!newName.trim()) return;
    try {
      // Need to import updateSessionName from ipc.ts
      const { updateSessionName } = await import('../../services/ipc');
      await updateSessionName(this.sessionId, newName.trim());
      await this.loadSession();
    } catch (e) {
      this.error = String(e);
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
      return html`<div class="empty">Loading meeting session...</div>`;
    }

    // Assuming both tracks have the same session_name, use the first one
    const sessionName = this.recordings[0]?.session_name || this.sessionId;

    return html`
      <div style="display: flex; flex-direction: column; gap: 1rem; padding: 1rem;">
        <div style="display: flex; justify-content: space-between; align-items: center; padding-bottom: 0.5rem; border-bottom: 1px solid var(--border-color, #ccc);">
          <h2 style="margin: 0; color: var(--text-color, #333); display: flex; align-items: center; gap: 8px;">
            Meeting: 
            <span 
              contenteditable="true"
              style="padding: 2px 6px; border-radius: 4px; border: 1px solid transparent; outline: none; transition: border 0.2s;"
              @blur=${(e: Event) => this.saveSessionName((e.target as HTMLElement).innerText)}
              @keydown=${this.handleKeyDown}
              @focus=${(e: Event) => (e.target as HTMLElement).style.border = '1px solid var(--accent, #89b4fa)'}
              @focusout=${(e: Event) => (e.target as HTMLElement).style.border = '1px solid transparent'}
              title="Click to rename this meeting"
            >${sessionName}</span>
          </h2>
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
