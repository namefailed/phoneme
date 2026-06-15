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
  const bar = document.querySelector<HTMLElement>("ph-header-bar");
  const dur =
    parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--pane-anim")) || 0;
  // Animations off (or no bar mounted): just flip the class — CSS does the rest.
  if (dur <= 0 || !bar) {
    body.classList.toggle("phoneme-hide-header", hide);
    return;
  }
  // Animate max-height over the bar's REAL (measured) height in BOTH directions.
  // A fixed `max-height: 160px` cap only *looks* animated while max-height crosses
  // the actual content height (~one row), so SHOW (0 → 160) finished its visible
  // reveal in the first third of the duration and read as an instant snap, while
  // HIDE (160 → 0) looked fine. Driving max-height between 0 and `scrollHeight`
  // (the unclipped content height — correct even while collapsed) makes the whole
  // eased curve visible and symmetric on/off. The inline value is released after
  // the transition so the CSS cap (or the collapsed rule) resumes.
  body.classList.add("phoneme-header-anim");
  const full = bar.scrollHeight;
  if (hide) {
    bar.style.maxHeight = `${full}px`;
    void bar.offsetHeight; // commit the start height before transitioning to 0
    body.classList.add("phoneme-hide-header");
    bar.style.maxHeight = "0px";
  } else {
    bar.style.maxHeight = "0px";
    body.classList.remove("phoneme-hide-header");
    void bar.offsetHeight; // commit 0 before transitioning up to the real height
    bar.style.maxHeight = `${full}px`;
  }
  window.setTimeout(() => {
    body.classList.remove("phoneme-header-anim");
    bar.style.maxHeight = ""; // release: CSS 160 cap (shown) or the :hidden rule (0)
  }, dur + 80);
}

/** Whether the top bar is currently hidden (the `<body>` class is the single
 *  source of truth — zen modes snapshot this to restore the user's choice). */
export function isHeaderHidden(): boolean {
  return document.body.classList.contains("phoneme-hide-header");
}
