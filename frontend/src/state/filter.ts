/**
 * The shared library-filter state: ONE `filterStore` that every filtering
 * surface (header search box, sidebar kind/tag rows, saved searches, the
 * "More like this" pill) writes to, and that RecordingsList re-queries the
 * daemon on whenever it changes. This module owns the UI-side filter shape
 * and its translation to the daemon's wire `ListFilter`.
 */
import { Store } from "./store";
import type { ListFilter } from "../services/ipc";

/** Library type-filter: all recordings, single-track only, meetings only,
 *  in-place dictations (typed straight into another app), starred (favorites),
 *  or pinned. */
export type RecordingKind = "all" | "single" | "meeting" | "in_place" | "favorite" | "pinned";

/** Tag-presence filter, independent of `kind` and `tag_id`: only recordings
 *  with ≥1 tag, only recordings with 0 tags, or no constraint. Powers the
 *  sidebar's toggleable "Untagged" / "Tagged" rows. */
export type TagState = "tagged" | "untagged" | null;

/**
 * The library filter as the UI models it: the daemon's `ListFilter` extended
 * with UI-only state that is never sent over the wire (`semantic`, the
 * like-mode fields) and with `kind`/`favorite`/`pinned` re-modelled as one
 * single choice. Convert with `toWireFilter` before calling `listRecordings`.
 */
export type UiFilter = Omit<ListFilter, "kind" | "favorite" | "pinned"> & {
  semantic?: boolean;
  /** Library type-filter as the UI models it (one dropdown of choices).
   *  `toWireFilter` maps it onto the daemon's `kind` / `favorite` / `pinned`
   *  fields. */
  kind?: RecordingKind;
  /** Tag-presence filter, independent of `kind`/`tag_id`. `toWireFilter` maps it
   *  onto the daemon's `tagged` flag (true = tagged only, false = untagged only). */
  tagState?: TagState;
  /** Low-confidence filter (confidence-driven re-do): show only recordings flagged
   *  low confidence. UI-only as a boolean — `toWireFilter` turns it into the
   *  daemon's numeric `low_confidence_below` using the configured threshold passed
   *  in, so the SQL comparison and the badge agree. */
  lowConfidence?: boolean;
  /** UI-only "More like this" mode: when set, the list shows recordings
   *  semantically similar to this recording (by its stored vectors) instead
   *  of the normal filtered list. Takes precedence over `search`. */
  like_id?: string | null;
  /** Human label for the like-mode pill (the source's title); falls back to
   *  `like_id` when absent. Display-only. */
  like_label?: string | null;
  /** Display label for the entity-filter pill (the entity's `kind` group name,
   *  e.g. "Person"). Display-only — `entity_value` / `entity_kind` carry the
   *  actual filter. Mirrors `like_label`. */
  entity_label?: string | null;
};

/** The one shared library-filter store. `{}` = no filters (the whole library).
 *  Update immutably (`filterStore.set({ ...filterStore.get(), … })`) so
 *  subscribers — primarily RecordingsList's re-query — actually fire. */
export const filterStore = new Store<UiFilter>({});

/**
 * Translate the UI filter into the daemon's wire `ListFilter`: drop the
 * UI-only state (semantic toggle, like-mode) and map the Library `kind` onto the
 * server-side `kind` / `favorite` / `pinned` fields, so the daemon filters in
 * SQL *before* pagination. Filtering client-side after pagination made pages of
 * the chosen kind come back mostly (or entirely) empty.
 */
export function toWireFilter(f: UiFilter, lowConfidenceThreshold?: number): ListFilter {
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
  else if (f.kind === "in_place") wire.in_place = true;
  else if (f.kind === "favorite") wire.favorite = true;
  else if (f.kind === "pinned") wire.pinned = true;
  // Tag-presence ("All Tags" / "Untagged") rides its own flag, independent of
  // the kind/tag_id filters it combines with.
  if (f.tagState === "tagged") wire.tagged = true;
  else if (f.tagState === "untagged") wire.tagged = false;
  // Entity facet filter (sidebar browse-by-entity): pass the exact value through,
  // plus its kind so the same surface text under two kinds stays distinct. The
  // `entity_label` is UI-only (the pill text) and never goes over the wire.
  if (f.entity_value) {
    wire.entity_value = f.entity_value;
    if (f.entity_kind) wire.entity_kind = f.entity_kind;
  }
  // Task-presence filter (sidebar Tasks section): pass the state token through so
  // the daemon narrows to recordings with open / any tasks via its `tasks`
  // subquery. Unrecognized values are ignored daemon-side.
  if (f.task_state) wire.task_state = f.task_state;
  // Low-confidence: the UI boolean becomes the daemon's numeric threshold so the
  // SQL `mean_confidence < t` comparison uses the configured value. Without a
  // threshold (config not loaded) the filter is simply dropped rather than
  // guessing a number — the list shows everything, never an empty page.
  if (f.lowConfidence && typeof lowConfidenceThreshold === "number") {
    wire.low_confidence_below = lowConfidenceThreshold;
  }
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

/**
 * Apply the cross-recording entity filter: narrow the list to recordings that
 * mention this entity, the entity counterpart of clicking a sidebar tag. Pass the
 * `kind` so the same surface text under two kinds stays distinct; `label` is the
 * human group name shown in the header pill. Clicking the already-active entity
 * row toggles it off (mirrors the tag rows). Leaves the other filter dimensions
 * alone so it COMBINES with kind/date/etc.
 */
export function applyEntityFilter(value: string, kind?: string | null, label?: string | null): void {
  const f = filterStore.get();
  // Re-clicking the active entity (same value + kind) turns it off, back to all.
  const sameKind = (f.entity_kind ?? null) === (kind ?? null);
  if (f.entity_value === value && sameKind) {
    clearEntityFilter();
    return;
  }
  filterStore.set({
    ...f,
    entity_value: value,
    entity_kind: kind ?? null,
    entity_label: label?.trim() || null,
  });
}

/** Clear the entity filter and return to the normal list. */
export function clearEntityFilter(): void {
  filterStore.set({
    ...filterStore.get(),
    entity_value: null,
    entity_kind: null,
    entity_label: null,
  });
}

/**
 * Apply the cross-recording task-presence filter: narrow the list to recordings
 * with at least one open task (`"has_open"`) or any extracted task
 * (`"has_tasks"`), the task counterpart of {@link applyEntityFilter}. Clicking the
 * already-active state toggles it off. Leaves the other filter dimensions alone so
 * it COMBINES with kind/date/etc.
 */
export function applyTaskFilter(state: "has_open" | "has_tasks"): void {
  const f = filterStore.get();
  if (f.task_state === state) {
    clearTaskFilter();
    return;
  }
  filterStore.set({ ...f, task_state: state });
}

/** Clear the task-presence filter and return to the normal list. */
export function clearTaskFilter(): void {
  filterStore.set({ ...filterStore.get(), task_state: null });
}
