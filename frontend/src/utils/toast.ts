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

/** Toast flavor: picks the icon, color, and default auto-dismiss delay. */
export type ToastType = "success" | "error" | "info" | "warning";

const AUTO_DISMISS_MS: Record<ToastType, number> = {
  success: 3000,
  info: 3500,
  warning: 6000,
  // Errors used to persist forever; now they get a long-but-finite window.
  // Hovering pauses the clock (see below), so "I was reading it" never loses
  // the message — and callers can still pass duration 0 for a sticky toast.
  error: 10000,
};

/** Most toasts visible at once — a burst (e.g. bulk re-run failures) drops the
 *  oldest instead of wallpapering the screen. */
const MAX_STACK = 6;

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

  // Cap the stack — drop the oldest toast(s) first (the first DOM children).
  while (container.children.length >= MAX_STACK) {
    container.firstElementChild?.remove();
  }

  const toast = document.createElement("div");
  toast.className = `toast toast-${type}`;
  toast.setAttribute("role", "alert");
  toast.innerHTML = `
    <span class="toast-icon" aria-hidden="true">${ICONS[type]}</span>
    <span class="toast-msg">${escapeHtml(message)}</span>
    <button class="toast-close" aria-label="Dismiss notification">×</button>
    ${dismissAfter > 0 ? `<span class="toast-countdown" style="animation-duration:${dismissAfter}ms"></span>` : ""}
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
    attachPausableTimer(toast, dismissAfter, dismiss);
  }
}

/**
 * Auto-dismiss `el` after `totalMs`, but PAUSE the clock while the pointer is
 * over it — reading or aiming for a button must never race the timeout. The
 * countdown bar pauses in sync via CSS (`.toast:hover .toast-countdown`).
 * Resuming always grants a small grace so the toast can't vanish the instant
 * the pointer leaves.
 */
function attachPausableTimer(el: HTMLElement, totalMs: number, onExpire: () => void) {
  let remaining = totalMs;
  let startedAt = Date.now();
  let timer: ReturnType<typeof setTimeout> | null = setTimeout(onExpire, remaining);
  el.addEventListener("mouseenter", () => {
    if (timer) {
      clearTimeout(timer);
      timer = null;
      remaining -= Date.now() - startedAt;
    }
  });
  el.addEventListener("mouseleave", () => {
    if (!el.isConnected) return;
    startedAt = Date.now();
    timer = setTimeout(onExpire, Math.max(800, remaining));
  });
}

/**
 * Show a toast with an action button (e.g. "Undo") plus a thin countdown bar.
 *
 * Three exits, each fires exactly one callback:
 *   • the action button  → `onAction`  (and NOT onExpire)
 *   • the × / auto-timeout → `onExpire`
 * Used by the undoable-delete flow: the row is hidden immediately, the real
 * delete is deferred to `onExpire`, and `onAction` cancels it.
 */
export function showActionToast(opts: {
  message: string;
  actionLabel: string;
  onAction: () => void;
  onExpire?: () => void;
  durationMs?: number;
  icon?: string;
}): void {
  const { message, actionLabel, onAction, onExpire, durationMs = 6000, icon = "i" } = opts;
  const container = getContainer();

  const toast = document.createElement("div");
  toast.className = "toast toast-info toast-action-toast";
  toast.setAttribute("role", "alert");
  toast.innerHTML = `
    <span class="toast-icon" aria-hidden="true">${escapeHtml(icon)}</span>
    <span class="toast-msg">${escapeHtml(message)}</span>
    <button class="toast-action"></button>
    <button class="toast-close" aria-label="Dismiss notification">×</button>
    <span class="toast-countdown" style="animation-duration:${durationMs}ms"></span>
  `;
  toast.querySelector<HTMLButtonElement>(".toast-action")!.textContent = actionLabel;

  let settled = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const removeEl = () => {
    if (!toast.isConnected) return;
    toast.classList.add("toast-out");
    toast.addEventListener("animationend", () => toast.remove(), { once: true });
  };
  const finish = (cb?: () => void) => {
    if (settled) return;
    settled = true;
    if (timer) clearTimeout(timer);
    removeEl();
    cb?.();
  };

  toast.querySelector<HTMLButtonElement>(".toast-action")!
    .addEventListener("click", () => finish(onAction));
  toast.querySelector<HTMLButtonElement>(".toast-close")!
    .addEventListener("click", () => finish(onExpire));

  container.appendChild(toast);
  // Hover pauses the undo window — aiming for the button must not race it.
  // `finish` is idempotent, so the pausable timer can never double-fire.
  let remaining = durationMs;
  let startedAt = Date.now();
  timer = setTimeout(() => finish(onExpire), remaining);
  toast.addEventListener("mouseenter", () => {
    if (timer) {
      clearTimeout(timer);
      timer = undefined;
      remaining -= Date.now() - startedAt;
    }
  });
  toast.addEventListener("mouseleave", () => {
    if (settled) return;
    startedAt = Date.now();
    timer = setTimeout(() => finish(onExpire), Math.max(800, remaining));
  });
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}
