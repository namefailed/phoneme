import { tailLog } from "../../services/ipc";
import { errText } from "../../utils/error";
import { closeModalOverlay } from "../../utils/modalAnim";
import "../modal.css";

/** Logs the viewer can show — kept in sync with the backend `tail_log` allowlist. */
const LOGS: { name: string; label: string }[] = [
  { name: "hook.log", label: "Hook log" },
  { name: "daemon.log", label: "Daemon log" },
];

const MAX_LINES = 400;

/** Teardown for the currently-open viewer (removes its document keydown listener
 *  and overlay). Ensures only one instance — and one listener — is ever live, so
 *  rapid re-opens can't accumulate handlers on `document`. */
let activeClose: (() => void) | null = null;

/**
 * Read-only log viewer modal (Settings → Destination & Integrations). Tails the
 * last few hundred lines of `hook.log` (or the daemon log) so a hook that
 * silently does nothing can be debugged without leaving the app. Plain-DOM
 * `.modal-overlay` so the global keyboard layers stand down and Escape closes
 * the viewer, never the recording behind it.
 */
export function openLogViewer(initial: string = "hook.log"): void {
  // Tear down any prior instance first (its listener + overlay), so opening the
  // viewer twice can't leave a stale keydown handler bound to `document`.
  activeClose?.();
  document.querySelector(".log-viewer-overlay")?.remove();

  const overlay = document.createElement("div");
  overlay.className = "modal-overlay log-viewer-overlay";
  overlay.innerHTML = `
    <div class="modal-dialog log-viewer-dialog" role="dialog" aria-modal="true" aria-label="Log viewer"
         style="width: min(820px, 92vw); max-height: 82vh; display: flex; flex-direction: column;">
      <div class="modal-header" style="display: flex; align-items: center; gap: 10px;">
        <h3 class="modal-title" style="margin: 0;">Logs</h3>
        <select class="log-select" style="background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; padding: 4px 8px; font-size: 0.8571rem; color: var(--fg-default); cursor: pointer;">
          ${LOGS.map((l) => `<option value="${l.name}"${l.name === initial ? " selected" : ""}>${l.label}</option>`).join("")}
        </select>
        <button class="inline-button log-refresh" title="Reload the last ${MAX_LINES} lines">Refresh</button>
        <span class="log-status" style="font-size: 0.7857rem; color: var(--fg-faded); margin-left: auto;"></span>
      </div>
      <pre class="log-body" style="flex: 1; min-height: 200px; overflow: auto; margin: 12px 0 0; padding: 10px 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 4px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 0.7857rem; line-height: 1.5; white-space: pre-wrap; word-break: break-word; color: var(--fg-default);"></pre>
      <div class="modal-actions" style="margin-top: 14px;">
        <button class="modal-btn modal-btn-primary log-close">Close</button>
      </div>
    </div>
  `;

  const select = overlay.querySelector<HTMLSelectElement>(".log-select")!;
  const body = overlay.querySelector<HTMLPreElement>(".log-body")!;
  const status = overlay.querySelector<HTMLElement>(".log-status")!;

  const close = () => {
    document.removeEventListener("keydown", onKey);
    closeModalOverlay(overlay, () => {
      overlay.remove();
      if (activeClose === close) activeClose = null;
    });
  };
  activeClose = close;
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };

  async function load() {
    const name = select.value;
    status.textContent = "Loading…";
    try {
      const text = await tailLog(name, MAX_LINES);
      if (text.trim()) {
        body.textContent = text;
        body.scrollTop = body.scrollHeight; // newest at the bottom
        status.textContent = `last ${Math.min(MAX_LINES, text.split("\n").length)} lines`;
      } else {
        body.textContent = `No ${name} yet. It appears once a hook (or the daemon) has written to it.\nFull logs live in %LOCALAPPDATA%\\phoneme\\logs`;
        status.textContent = "empty";
      }
    } catch (e) {
      body.textContent = errText(e);
      status.textContent = "error";
    }
  }

  select.addEventListener("change", () => void load());
  overlay.querySelector(".log-refresh")?.addEventListener("click", () => void load());
  overlay.querySelector(".log-close")?.addEventListener("click", close);
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) close();
  });
  document.addEventListener("keydown", onKey);

  document.body.appendChild(overlay);
  void load();
}
