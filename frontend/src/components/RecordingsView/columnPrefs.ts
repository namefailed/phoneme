/**
 * Per-device display prefs for the two "quick-action" columns that sit apart
 * from the reorderable column set: Favorites (⭐) and Pinned (📌). Turning one
 * off hides BOTH its list column and its Library sidebar section, in one switch.
 *
 * Kept in localStorage (like the per-column widths) rather than the daemon
 * `interface.visible_columns`: it's a per-device display choice that defaults
 * ON, and a dedicated key sidesteps the "is an absent column off, or just a
 * config that predates this option?" ambiguity that visible-columns membership
 * would carry. Setters fire `phoneme:display-prefs-changed` so the list and the
 * sidebar re-render live, no save round-trip needed.
 */
export const DISPLAY_PREFS_EVENT = "phoneme:display-prefs-changed";

const FAV_KEY = "phoneme.showFavorites";
const PIN_KEY = "phoneme.showPinned";

function read(key: string): boolean {
  try {
    // Default ON: only an explicit "0" turns it off (absent/unset = shown).
    return localStorage.getItem(key) !== "0";
  } catch {
    return true;
  }
}

function write(key: string, on: boolean): void {
  try {
    localStorage.setItem(key, on ? "1" : "0");
  } catch {
    /* localStorage may be unavailable; the toggle just won't persist */
  }
  window.dispatchEvent(new Event(DISPLAY_PREFS_EVENT));
}

/** Whether the Favorites column + Library section are shown (default true). */
export const showFavorites = (): boolean => read(FAV_KEY);
/** Whether the Pinned column + Library section are shown (default true). */
export const showPinned = (): boolean => read(PIN_KEY);
export const setShowFavorites = (on: boolean): void => write(FAV_KEY, on);
export const setShowPinned = (on: boolean): void => write(PIN_KEY, on);
