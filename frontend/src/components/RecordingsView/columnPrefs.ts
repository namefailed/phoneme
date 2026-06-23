/**
 * Per-device display prefs that sit apart from the reorderable column set:
 * - the two "quick-action" columns Favorites (⭐) and Pinned (📌) — off hides
 *   BOTH the list column and its Library sidebar section (default OFF);
 * - whether the Tags / Tasks / Entities sidebar sections show at all
 *   (default ON — they've always shown).
 *
 * Kept in localStorage (like the per-column widths and the sections' fold state)
 * rather than the daemon config: these are per-device display choices, and a
 * dedicated key sidesteps the "is an absent value off, or just a config that
 * predates this option?" ambiguity. Setters fire `phoneme:display-prefs-changed`
 * so the list and the sidebar re-render live, no save round-trip needed.
 */
export const DISPLAY_PREFS_EVENT = "phoneme:display-prefs-changed";

const FAV_KEY = "phoneme.showFavorites";
const PIN_KEY = "phoneme.showPinned";

function read(key: string): boolean {
  try {
    // Default OFF: only an explicit "1" shows it (absent/unset = hidden).
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
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

/** Whether the Favorites column + Library section are shown (default false). */
export const showFavorites = (): boolean => read(FAV_KEY);
/** Whether the Pinned column + Library section are shown (default false). */
export const showPinned = (): boolean => read(PIN_KEY);
export const setShowFavorites = (on: boolean): void => write(FAV_KEY, on);
export const setShowPinned = (on: boolean): void => write(PIN_KEY, on);

const SEC_TAGS_KEY = "phoneme.showSidebarTags";
const SEC_TASKS_KEY = "phoneme.showSidebarTasks";
const SEC_ENTITIES_KEY = "phoneme.showSidebarEntities";

/** Default ON (the sections have always shown): only an explicit "0" hides one. */
function readOn(key: string): boolean {
  try {
    return localStorage.getItem(key) !== "0";
  } catch {
    return true;
  }
}

/** Whether the Tags sidebar section is shown (default true). */
export const showSidebarTags = (): boolean => readOn(SEC_TAGS_KEY);
/** Whether the Tasks sidebar section is shown (default true). */
export const showSidebarTasks = (): boolean => readOn(SEC_TASKS_KEY);
/** Whether the Entities sidebar section is shown (default true). */
export const showSidebarEntities = (): boolean => readOn(SEC_ENTITIES_KEY);
export const setShowSidebarTags = (on: boolean): void => write(SEC_TAGS_KEY, on);
export const setShowSidebarTasks = (on: boolean): void => write(SEC_TASKS_KEY, on);
export const setShowSidebarEntities = (on: boolean): void => write(SEC_ENTITIES_KEY, on);
