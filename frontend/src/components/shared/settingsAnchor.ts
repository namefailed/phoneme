/**
 * Records the on-screen position of the header's ⚙ Settings button at the moment
 * Settings is opened, so the Settings view's floating ⚙ Settings button can be
 * placed at the *exact* same spot — opening Settings then never makes the button
 * appear to jump. Cleared/ignored when Settings is reached by another path (the
 * floating button falls back to its CSS default position then).
 */
export type SettingsAnchor = { top: number; left: number; width: number; height: number };

let anchor: SettingsAnchor | null = null;

/** Capture the header ⚙ button's viewport rect (App calls this just before
 *  mounting Settings, while the header is still visible). */
export function setSettingsAnchor(rect: SettingsAnchor): void {
  anchor = rect;
}

/** The captured rect, or null when Settings was reached without one (keyboard
 *  shortcut / deep link) — the floating button then uses its CSS position. */
export function getSettingsAnchor(): SettingsAnchor | null {
  return anchor;
}
