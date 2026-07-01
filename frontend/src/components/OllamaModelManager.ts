/**
 * "Manage local models" — a small modal for the local Ollama install.
 *
 * Lists the installed models with their on-disk size, deletes one (behind a
 * confirm), and pulls a new one with a live progress bar. It's the
 * outside-the-wizard counterpart to the first-run wizard's model pull, reachable
 * from the Models picker (Post-processing tab) and Settings → Post-Processing —
 * so users can grow/shrink their local model set any time, not just at setup.
 *
 * Pure DOM + the shared `.modal-*` styles (the house self-removing-overlay
 * idiom), backed by services/ollamaModels.ts (the `ollama_*` Tauri commands).
 * No daemon round-trip: model management is a local-Ollama concern, the same
 * plane the wizard's pull already uses.
 */
import { closeModalOverlay } from "../utils/modalAnim";
import { showToast } from "../utils/toast";
import { errText } from "../utils/error";
import { escapeHtml, escapeAttr } from "../utils/format";
import { confirmDialog } from "./confirmDialog";
import {
  listInstalledOllamaModels,
  deleteOllamaModel,
  pullOllamaModel,
  formatBytes,
  type OllamaInstalledModel,
} from "../services/ollamaModels";
import { curatedCleanupModelIds } from "../data/curatedModels";

/**
 * Open the local-model manager. Resolves when the modal closes (no payload —
 * callers that care about a changed model set re-fetch on their own; the daemon
 * picks up whatever's installed at run time). The house self-removing idiom:
 * build the overlay, await close, remove.
 */
export function openOllamaModelManager(): Promise<void> {
  return new Promise((resolve) => {
    // Never stack two managers.
    document.querySelector(".ph-ollama-mgr")?.closest(".modal-overlay")?.remove();

    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    overlay.innerHTML = `
      <div class="modal-dialog ph-ollama-mgr" role="dialog" aria-modal="true" aria-labelledby="om-title" style="max-width: 560px; width: 92vw;">
        <div class="modal-header"><h3 class="modal-title" id="om-title">Manage local models</h3></div>

        <p class="modal-body" style="margin-bottom: 10px;">
          Models installed in your local <b>Ollama</b>. Pull new ones or delete ones you no longer use to free disk.
        </p>

        <div class="om-pull" style="display:flex; gap:8px; align-items:center; margin-bottom: 12px;">
          <input id="om-pull-input" class="mp-input" type="text" list="om-pull-suggest"
            placeholder="Model to pull, e.g. llama3.2:3b" style="flex:1;" autocomplete="off" />
          <datalist id="om-pull-suggest"></datalist>
          <button id="om-pull-btn" class="modal-btn modal-btn-primary" type="button">Pull</button>
        </div>

        <div id="om-progress" style="display:none; margin-bottom: 12px;">
          <div id="om-progress-label" style="font-size: 0.8214rem; color: var(--fg-muted); margin-bottom: 4px;"></div>
          <progress id="om-progress-bar" style="width:100%;"></progress>
        </div>

        <div id="om-list" class="om-list" style="max-height: 42vh; overflow:auto;"></div>

        <div class="modal-actions">
          <button id="om-close" class="modal-btn" type="button">Close</button>
        </div>
      </div>`;

    const listEl = overlay.querySelector<HTMLElement>("#om-list")!;
    const pullInput = overlay.querySelector<HTMLInputElement>("#om-pull-input")!;
    const pullBtn = overlay.querySelector<HTMLButtonElement>("#om-pull-btn")!;
    const progress = overlay.querySelector<HTMLElement>("#om-progress")!;
    const progressLabel = overlay.querySelector<HTMLElement>("#om-progress-label")!;
    const progressBar = overlay.querySelector<HTMLProgressElement>("#om-progress-bar")!;
    const suggest = overlay.querySelector<HTMLDataListElement>("#om-pull-suggest")!;

    // Seed the pull input's suggestions from the curated Ollama list.
    suggest.innerHTML = curatedCleanupModelIds("ollama")
      .map((id) => `<option value="${escapeHtml(id)}"></option>`)
      .join("");

    let pulling = false;

    const settle = () => {
      document.removeEventListener("keydown", onKey, true);
      closeModalOverlay(overlay, () => {
        overlay.remove();
        resolve();
      });
    };

    // Esc closes the manager, but never mid-pull (a half-finished pull would
    // keep running headless with no progress surface); capture phase so it
    // doesn't leak to the app-level Escape handler.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (!pulling) settle();
      }
    };

    const renderList = (models: OllamaInstalledModel[] | null, error?: string) => {
      if (error) {
        listEl.innerHTML = `<div class="om-empty" style="padding:16px; text-align:center; color: var(--err);">${escapeHtml(error)}</div>`;
        return;
      }
      if (!models) {
        listEl.innerHTML = `<div class="om-empty" style="padding:16px; text-align:center; color: var(--fg-muted);">Loading…</div>`;
        return;
      }
      if (models.length === 0) {
        listEl.innerHTML = `<div class="om-empty" style="padding:16px; text-align:center; color: var(--fg-muted);">No models installed yet. Pull one above.</div>`;
        return;
      }
      listEl.innerHTML = models
        .map(
          (m) => `
        <div class="om-row" data-name="${escapeAttr(m.name)}"
          style="display:flex; align-items:center; gap:10px; padding:8px 6px; border-bottom:1px solid var(--border-subtle);">
          <div style="flex:1; min-width:0;">
            <div style="font-weight:600; overflow:hidden; text-overflow:ellipsis; white-space:nowrap;">${escapeHtml(m.name)}</div>
            <div style="font-size:0.75rem; color: var(--fg-faded);">${escapeHtml(formatBytes(m.size))}</div>
          </div>
          <button class="modal-btn modal-btn-danger om-del" data-name="${escapeAttr(m.name)}" type="button" title="Delete ${escapeAttr(m.name)}">Delete</button>
        </div>`,
        )
        .join("");

      listEl.querySelectorAll<HTMLButtonElement>(".om-del").forEach((btn) =>
        btn.addEventListener("click", () => deleteOne(btn.dataset.name!)),
      );
    };

    const refresh = async () => {
      renderList(null);
      try {
        const models = await listInstalledOllamaModels();
        renderList(models);
      } catch (e) {
        // The unreachable case is the common one (Ollama not running) — say so
        // plainly rather than dumping a transport error.
        const msg = errText(e);
        renderList([], /ollama/i.test(msg) ? "Ollama isn't reachable. Start Ollama, then reopen this." : `Couldn't list models: ${msg}`);
      }
    };

    const deleteOne = async (name: string) => {
      if (pulling) return;
      const ok = await confirmDialog({
        title: "Delete model?",
        body: `Remove "${name}" from your local Ollama? This frees its disk; you can pull it again later.`,
        confirmLabel: "Delete",
        danger: true,
      });
      if (!ok) return;
      try {
        await deleteOllamaModel(name);
        showToast(`Deleted ${name}`, "success");
        await refresh();
      } catch (e) {
        showToast(`Couldn't delete: ${errText(e)}`, "error");
      }
    };

    const setPulling = (on: boolean) => {
      pulling = on;
      pullBtn.disabled = on;
      pullInput.disabled = on;
      progress.style.display = on ? "" : "none";
      if (!on) {
        progressBar.removeAttribute("value");
        progressBar.removeAttribute("max");
        progressLabel.textContent = "";
      }
    };

    const doPull = async () => {
      const model = pullInput.value.trim();
      if (!model || pulling) return;
      setPulling(true);
      progressLabel.textContent = "Starting pull…";
      try {
        await pullOllamaModel(model, (p) => {
          progressLabel.textContent = p.status || "Pulling…";
          if (p.total && p.completed) {
            progressBar.max = p.total;
            progressBar.value = p.completed;
          } else {
            // Metadata phases report no byte counts — show an indeterminate bar.
            progressBar.removeAttribute("value");
          }
        });
        showToast(`Pulled ${model}`, "success");
        pullInput.value = "";
        await refresh();
      } catch (e) {
        showToast(`Pull failed: ${errText(e)}`, "error");
      } finally {
        setPulling(false);
      }
    };

    pullBtn.addEventListener("click", doPull);
    pullInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        doPull();
      }
    });

    overlay.addEventListener("click", (e) => {
      if (e.target === overlay && !pulling) settle();
    });
    // Guard Close the same as Esc + backdrop: don't dismiss mid-pull (which would
    // orphan the in-flight pull with no progress surface).
    overlay.querySelector<HTMLButtonElement>("#om-close")!.addEventListener("click", () => {
      if (!pulling) settle();
    });
    document.addEventListener("keydown", onKey, true);

    document.body.appendChild(overlay);
    pullInput.focus();
    void refresh();
  });
}
