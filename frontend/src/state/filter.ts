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

export type UiFilter = Omit<ListFilter, "kind" | "favorite"> & {
  semantic?: boolean;
  /** Library type-filter as the UI models it (one dropdown of four choices).
   *  `toWireFilter` maps it onto the daemon's `kind` / `favorite` fields. */
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
 * Translate the UI filter into the daemon's wire `ListFilter`: drop the
 * UI-only state (semantic toggle, like-mode) and map the four-way Library
 * `kind` onto the server-side `kind` / `favorite` fields, so the daemon
 * filters in SQL *before* pagination. Filtering client-side after pagination
 * made pages of the chosen kind come back mostly (or entirely) empty.
 */
export function toWireFilter(f: UiFilter): ListFilter {
  const wire: ListFilter = {
    limit: f.limit,
    offset: f.offset,
    since: f.since,
    until: f.until,
    status: f.status,
    search: f.search,
    tag_id: f.tag_id,
    sort_desc: f.sort_desc,
  };
  if (f.kind === "single" || f.kind === "meeting") wire.kind = f.kind;
  else if (f.kind === "favorite") wire.favorite = true;
  return wire;
}

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
