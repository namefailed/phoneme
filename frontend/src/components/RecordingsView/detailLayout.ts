// The detail pane's static shell markup — the header (title/meta/focus buttons),
// the waveform + mount slots, the transcript block with its peek boxes and the
// Views/Versions dropdowns, and the footer. Pure string builder: RecordingDetail
// drops this into its container, then mounts widgets and wires events against the
// known ids/classes. Lifted out of RecordingDetail.ts to keep the component an
// orchestrator (same spirit as detailGrid.ts).

import type { Recording } from "../../services/ipc";
import {
  formatDuration,
  statusToClass,
  statusLabel,
  escapeHtml,
  escapeAttr,
} from "../../utils/format";
import { showFavorites, showPinned } from "./columnPrefs";
import { readPlaybackSpeed } from "./ActionRow";
import { CHEVRON_SVG, EXPAND_SVG, CLOSE_SVG, formatDate, pipelineHtml } from "./detailMeta";

/** Build the detail pane's shell HTML for `r`. `use24h` formats the header date;
 *  `stats` is the pre-computed word-count summary for the footer. */
export function recordingShellHtml(r: Recording, use24h: boolean, stats: string): string {
  return `
      <div class="detail">
        <div class="detail-header" style="display: flex; justify-content: space-between; align-items: flex-start;">
          <div style="min-width: 0; flex: 1;">
            <div class="detail-title" id="detail-title" style="font-size: 1.2857rem; font-weight: 700; margin-bottom: 6px; cursor: text;" title="Click to edit the title — Enter saves, Esc cancels, empty resets to automatic"><span id="detail-title-text">${escapeHtml(r.title ?? formatDate(r.started_at, use24h))}</span></div>
            <div class="detail-meta" style="display: flex; align-items: center; gap: 8px;">
              <span id="detail-status" class="status-pill ${statusToClass(r.status)}">${statusLabel(r.status)}</span>
              <span id="detail-title-date" style="${r.title ? "" : "display: none;"}">${formatDate(r.started_at, use24h)}</span>
              <span>${formatDuration(r.duration_ms)}</span>
              <span class="rec-source ${r.track === "system" ? "rec-source--system" : "rec-source--mic"}" title="${r.track === "system" ? "System audio" : "Microphone"}"><span class="rec-source-ico">${r.track === "system" ? "🔊" : "🎤"}</span></span>
              ${r.in_place ? `<span class="detail-inplace-badge" title="Dictation — typed straight in place at your cursor">⌨ in-place</span>` : ""}
              ${
                r.detected_language
                  ? `<span class="detail-lang-badge" title="Spoken language the transcriber detected">🌐 ${escapeHtml(r.detected_language)}</span>`
                  : ""
              }
            </div>
          </div>
          <div style="display: flex; gap: 6px; align-items: center; flex-shrink: 0;">
            ${showFavorites() ? `<button class="detail-focus-btn rec-fav-btn ${r.favorite ? "on" : ""}" id="detail-fav" aria-label="${r.favorite ? "Unstar" : "Star"}" title="${r.favorite ? "Remove from Favorites" : "Add to Favorites"}">⭐</button>` : ""}
            ${showPinned() ? `<button class="detail-focus-btn rec-pin-btn ${r.pinned ? "on" : ""}" id="detail-pin" aria-label="${r.pinned ? "Unpin" : "Pin to top"}" title="${r.pinned ? "Unpin from the top of the library" : "Pin to the top of the library"}">📌</button>` : ""}
            <button class="detail-focus-btn" id="detail-similar" aria-label="More like this" title="More like this — fill the list with recordings about similar things"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg></button>
            <span aria-hidden="true" style="width: 1px; align-self: stretch; margin: 2px 2px; background: var(--border-subtle);"></span>
            <button class="detail-focus-btn" id="detail-focus" aria-label="Toggle focus mode" title="Focus mode — hide the recordings list and edit full-width">${EXPAND_SVG}</button>
            <button class="detail-focus-btn" id="detail-close" aria-label="Close recording" title="Close — back to the recordings list">${CLOSE_SVG}</button>
          </div>
        </div>
        <div class="waveform" id="wf-${r.id}"><span class="wf-speed-badge" id="wf-speed-${r.id}" title="Playback speed">${readPlaybackSpeed()}×</span></div>
        <div id="actions"></div>
        <div id="clip-export"></div>
        <div id="tags"></div>
        <div class="transcript-block">
          <div id="editor" style="flex: 1; display: flex; flex-direction: column; min-height: 0;"></div>
          <div id="original-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div id="unedited-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div id="summary-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div id="timeline-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 4px;"></div>
          <div id="synced-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div id="chapters-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 4px;"></div>
          <div class="transcript-history">
            <div class="th-group th-left">
              <button class="view-btn" id="rename-speakers" style="display: none;" title="Rename the diarized speakers (Speaker 1 → a name)">🏷️ Speakers</button>
            </div>
            <div class="th-group th-right">
              <span class="th-dropdown">
                <button class="view-btn th-trigger" id="views-trigger" aria-haspopup="menu" aria-expanded="false" title="Alternate views of this recording — summary, timeline, synced words">Views ${CHEVRON_SVG}</button>
                <div class="th-menu th-menu--right" id="views-menu" role="menu" hidden>
                  <button class="view-btn th-menu-item" id="view-summary" title="AI summary of this recording">📝 Summary</button>
                  <button class="view-btn th-menu-item" id="view-timeline" title="The transcript as a clickable timeline — click a line to jump playback there">🕒 Timeline</button>
                  <button class="view-btn th-menu-item" id="view-synced" title="The machine transcript as clickable words — click any word to jump playback there; the word under the playhead stays highlighted (read-only)">🔤 Synced</button>
                  <button class="view-btn th-menu-item" id="view-chapters" title="Topic chapters — click a chapter to jump playback there; the chapter under the playhead stays highlighted">🗂 Chapters</button>
                </div>
              </span>
              <span class="th-dropdown">
                <button class="view-btn th-trigger" id="versions-trigger" aria-haspopup="menu" aria-expanded="false" title="Other versions of this transcript — compare, raw machine, pre-edit">Versions ${CHEVRON_SVG}</button>
                <div class="th-menu th-menu--right" id="versions-menu" role="menu" hidden>
                  <button class="view-btn th-menu-item" id="view-compare" title="Compare any two transcript versions side by side">🆚 Compare</button>
                  <button class="view-btn th-menu-item" id="view-original" title="The raw machine transcript, before AI cleanup">📃 Original</button>
                  <button class="view-btn th-menu-item" id="view-unedited" title="The transcript as transcribed + cleaned, before you edited it">📄 Unedited</button>
                </div>
              </span>
            </div>
          </div>
        </div>
        <div id="insights"></div>
        <div class="notes-block" style="margin-top: 10px; border-top: 1px solid var(--border-subtle); padding-top: 12px;">
          <div id="notes-editor"></div>
        </div>
        <div class="detail-footer">
          <span id="detail-pipeline-host">${pipelineHtml(r)}</span>
          <span id="detail-stats">${stats}</span>
          <span class="detail-path" id="detail-reveal-path" role="button" tabindex="0" style="cursor: pointer; text-decoration: underline dotted; text-underline-offset: 2px;" title="Reveal in file explorer — ${escapeAttr(r.audio_path)}">${escapeHtml(r.audio_path)}</span>
        </div>
      </div>
    `;
}
