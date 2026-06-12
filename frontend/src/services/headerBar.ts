/**
 * Header (top bar) show/hide with the shared pane animation.
 *
 * The bar collapses via max-height on the same curve/duration as the panes
 * (--pane-anim). Clipping is applied ONLY while hidden or mid-animation
 * (body.phoneme-header-anim): a permanent overflow:hidden on ph-header-bar
 * would clip its own dropdown menus (Settings, record-mode), which hang below
 * the bar's box. Used by Ctrl+/ (keyboard.ts) and the zen/focus modes
 * (RecordingsView), so every path animates identically.
 */
export function setHeaderHidden(hide: boolean) {
  const body = document.body;
  if (body.classList.contains("phoneme-hide-header") === hide) return;
  const dur =
    parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--pane-anim")) || 0;
  if (dur > 0) {
    body.classList.add("phoneme-header-anim");
    window.setTimeout(() => body.classList.remove("phoneme-header-anim"), dur + 80);
  }
  body.classList.toggle("phoneme-hide-header", hide);
}

export function isHeaderHidden(): boolean {
  return document.body.classList.contains("phoneme-hide-header");
}
