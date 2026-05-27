/**
 * Lightweight singleton toast/snackbar notification system.
 *
 * Usage:
 *   import { showToast } from "../utils/toast";
 *   showToast("Saved!", "success");
 *   showToast("Failed to connect", "error");   // persists until dismissed
 *
 * The toast container (#toast-container) must be appended to <body> once at
 * app startup — App.ts handles that. The container intentionally lives outside
 * .app-shell so overflow:hidden on #main never clips it.
 */

export type ToastType = "success" | "error" | "info" | "warning";

const AUTO_DISMISS_MS: Record<ToastType, number> = {
  success: 3000,
  info: 3500,
  warning: 5000,
  error: 0, // errors persist until the user dismisses them
};

const ICONS: Record<ToastType, string> = {
  success: "✓",
  error: "✕",
  warning: "⚠",
  info: "i",
};

/** Returns (or lazily creates) the fixed-position toast container element. */
function getContainer(): HTMLElement {
  let el = document.getElementById("toast-container");
  if (!el) {
    el = document.createElement("div");
    el.id = "toast-container";
    document.body.appendChild(el);
  }
  return el;
}

/**
 * Show a toast notification.
 *
 * @param message   Text to display. HTML is escaped.
 * @param type      Visual style — success | error | info | warning.
 * @param duration  Override auto-dismiss delay in ms. Pass 0 to persist.
 */
export function showToast(
  message: string,
  type: ToastType = "info",
  duration?: number,
): void {
  const container = getContainer();
  const dismissAfter = duration ?? AUTO_DISMISS_MS[type];

  const toast = document.createElement("div");
  toast.className = `toast toast-${type}`;
  toast.setAttribute("role", "alert");
  toast.innerHTML = `
    <span class="toast-icon" aria-hidden="true">${ICONS[type]}</span>
    <span class="toast-msg">${escapeHtml(message)}</span>
    <button class="toast-close" aria-label="Dismiss notification">×</button>
  `;

  const dismiss = () => {
    if (!toast.isConnected) return;
    toast.classList.add("toast-out");
    toast.addEventListener("animationend", () => toast.remove(), { once: true });
  };

  toast.querySelector<HTMLButtonElement>(".toast-close")!
    .addEventListener("click", dismiss);

  container.appendChild(toast);

  if (dismissAfter > 0) {
    setTimeout(dismiss, dismissAfter);
  }
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}
