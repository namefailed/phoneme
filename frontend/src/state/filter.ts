import { Store } from "./store";
import type { ListFilter } from "../services/ipc";

/**
 * Extends ListFilter with UI-only state that isn't sent to the daemon.
 * `_timePreset` tracks which named preset is active in the time dropdown so
 * the select can restore its selected value after a re-render.
 */
/** Library type-filter: all recordings, single-track only, meetings only, or
 *  starred (favorites). */
export type RecordingKind = "all" | "single" | "meeting" | "favorite";

export type UiFilter = ListFilter & {
  semantic?: boolean;
  /** UI-only: filter the list by recording type (client-side, on meeting_id). */
  kind?: RecordingKind;
  /** UI-only "More like this" mode: when set, the list shows recordings
   *  semantically similar to this recording (by its stored vectors) instead
   *  of the normal filtered list. Takes precedence over `search`. */
  like_id?: string | null;
  /** Human label for the like-mode pill (the source's title); falls back to
   *  `like_id` when absent. Display-only. */
  like_label?: string | null;
};

export const filterStore = new Store<UiFilter>({});

/**
 * Switch the recordings list into "More like this" mode for one recording:
 * the list re-queries as a similarity search seeded by that recording's
 * stored vectors, and the header search box becomes a `~similar:` pill whose
 * ✕ returns to the normal list. Clears any text search so leaving like-mode
 * lands back on the plain library view.
 */
export function applyMoreLikeThis(id: string, label?: string | null): void {
  filterStore.set({
    ...filterStore.get(),
    search: null,
    like_id: id,
    like_label: label?.trim() || null,
  });
}

/** Leave "More like this" mode and return to the normal (unfiltered) list. */
export function clearMoreLikeThis(): void {
  filterStore.set({ ...filterStore.get(), like_id: null, like_label: null });
}
