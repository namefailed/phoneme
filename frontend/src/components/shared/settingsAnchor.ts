/**
 * Records the on-screen position of the header's ⚙ Settings button at the moment
 * Settings is opened, so the Settings view's floating ⚙ Settings button can be
 * placed at the *exact* same spot — opening Settings then never makes the button
 * appear to jump. Cleared/ignored when Settings is reached by another path (the
 * floating button falls back to its CSS default position then).
 */
export type SettingsAnchor = { top: number; left: number; width: number; height: number };

let anchor: SettingsAnchor | null = null;

export function setSettingsAnchor(rect: SettingsAnchor): void {
  anchor = rect;
}

export function getSettingsAnchor(): SettingsAnchor | null {
  return anchor;
}
