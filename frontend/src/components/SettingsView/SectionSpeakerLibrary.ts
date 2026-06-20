import { escapeHtml, escapeAttr } from "../../utils/format";
import {
  listNamedVoices,
  renameNamedVoice,
  mergeNamedVoices,
  forgetNamedVoice,
  type NamedVoice,
} from "../../services/ipc";
import { showToast } from "../../utils/toast";

/**
 * Speaker Library manager (Settings → Diarization). Lists the voices you've named
 * across recordings (#9) — voices are enrolled implicitly, so naming a speaker in
 * any recording's "Rename speakers" panel adds them here — and lets you rename a
 * voice, merge two that are the same person, or forget one. Forgetting only
 * removes the voiceprint from recognition; recordings and names already applied
 * are untouched.
 */
export class SectionSpeakerLibrary {
  private voices: NamedVoice[] = [];

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(private container: HTMLElement, _config: any) {
    this.render();
    void this.reload();
  }

  private async reload() {
    try {
      this.voices = await listNamedVoices();
    } catch {
      this.voices = [];
    }
    this.render();
  }

  private render() {
    const count = this.voices.length;
    const rows = this.voices
      .map((v) => {
        const others = this.voices.filter((o) => o.id !== v.id);
        const mergeOpts = others
          .map((o) => `<option value="${escapeAttr(o.id)}">${escapeHtml(o.name)}</option>`)
          .join("");
        return `
          <div class="spklib-row" data-id="${escapeAttr(v.id)}">
            <input class="spklib-name" type="text" value="${escapeAttr(v.name)}" aria-label="Voice name" />
            <span class="spklib-count" title="How many recordings this voice has been named in">${v.samples} clip${v.samples === 1 ? "" : "s"}</span>
            <div class="spklib-actions">
              ${
                others.length
                  ? `<select class="spklib-merge" aria-label="Merge ${escapeAttr(v.name)} into another voice">
                       <option value="">Merge into…</option>${mergeOpts}
                     </select>`
                  : ""
              }
              <button class="inline-button spklib-forget" title="Forget this voice (stops recognizing it)">Forget</button>
            </div>
          </div>`;
      })
      .join("");

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Speaker Library</h3>
        <p style="font-size: 0.8571rem; color:var(--fg-muted); margin:0 0 8px;">
          Voices you've named are remembered across recordings — when the same voice
          turns up again, Phoneme suggests the name in a recording's “Rename speakers”
          panel. Naming a speaker there is what adds them here.${
            count ? ` <b>${count}</b> voice${count === 1 ? "" : "s"}.` : ""
          }
        </p>

        <div class="settings-field" style="display:block;">
          <div class="spklib-list">
            ${
              rows ||
              `<div class="spklib-empty">No voices yet — name a speaker in a recording's “Rename speakers” panel and they'll be remembered here.</div>`
            }
          </div>
        </div>
      </div>

      <style>
        .spklib-list { display:flex; flex-direction:column; gap:8px; }
        .spklib-row {
          display:flex; align-items:center; gap:10px;
          border:1px solid var(--border-subtle); border-radius:8px;
          padding:8px 10px; background:var(--bg-surface);
        }
        .spklib-name {
          flex:1 1 auto; min-width:0;
          background:var(--bg-elevated); color:var(--fg-default);
          border:1px solid var(--border-subtle); border-radius:6px;
          padding:5px 9px; font-family:inherit; font-size:0.9286rem; font-weight:600;
        }
        .spklib-name:focus { outline:none; border-color:var(--accent); background:var(--bg-surface); }
        .spklib-count {
          flex:0 0 auto; font-size:0.7857rem; color:var(--fg-muted);
          white-space:nowrap; min-width:44px; text-align:right;
        }
        .spklib-actions { flex:0 0 auto; display:inline-flex; gap:6px; align-items:center; }
        .spklib-merge {
          height:28px; border-radius:6px; padding:0 6px; font-size:0.8214rem;
          background:var(--bg-elevated); color:var(--fg-default);
          border:1px solid var(--border-subtle); max-width:130px;
        }
        .spklib-empty { font-size:0.8571rem; color:var(--fg-muted); padding:6px 2px; }
      </style>`;

    this.wire();
  }

  private wire() {
    this.container.querySelectorAll<HTMLElement>(".spklib-row").forEach((row) => {
      const id = row.dataset.id!;
      const nameInput = row.querySelector<HTMLInputElement>(".spklib-name");
      nameInput?.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          nameInput.blur();
        } else if (e.key === "Escape") {
          e.preventDefault();
          nameInput.value = this.voices.find((v) => v.id === id)?.name ?? nameInput.value;
          nameInput.blur();
        }
      });
      nameInput?.addEventListener("blur", async () => {
        const name = nameInput.value.trim();
        const current = this.voices.find((v) => v.id === id);
        if (!name || name === current?.name) {
          nameInput.value = current?.name ?? "";
          return;
        }
        try {
          await renameNamedVoice(id, name);
          showToast("Voice renamed", "success");
          await this.reload();
        } catch (e) {
          showToast(`Couldn't rename: ${e}`, "error");
        }
      });

      row.querySelector<HTMLSelectElement>(".spklib-merge")?.addEventListener("change", async (e) => {
        const sel = e.target as HTMLSelectElement;
        const target = sel.value;
        if (!target) return;
        const from = this.voices.find((v) => v.id === id)?.name ?? "this voice";
        const into = this.voices.find((v) => v.id === target)?.name ?? "the other voice";
        if (
          !confirm(
            `Merge "${from}" into "${into}"?\n\nTheir voiceprints combine under "${into}", and "${from}" is removed from the library.`,
          )
        ) {
          sel.value = "";
          return;
        }
        try {
          await mergeNamedVoices(id, target);
          showToast(`Merged into "${into}"`, "success");
          await this.reload();
        } catch (err) {
          showToast(`Couldn't merge: ${err}`, "error");
        }
      });

      row.querySelector<HTMLButtonElement>(".spklib-forget")?.addEventListener("click", async () => {
        const name = this.voices.find((v) => v.id === id)?.name ?? "this voice";
        if (
          !confirm(
            `Forget "${name}"?\n\nPhoneme will stop recognizing this voice. Your recordings and any names you've already applied are unaffected — you can re-enroll by naming the speaker again.`,
          )
        ) {
          return;
        }
        try {
          await forgetNamedVoice(id);
          showToast(`Forgot "${name}"`, "success");
          await this.reload();
        } catch (e) {
          showToast(`Couldn't forget: ${e}`, "error");
        }
      });
    });
  }
}
