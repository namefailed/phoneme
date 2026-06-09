import { Store } from "./store";
import type { ListFilter } from "../services/ipc";

/**
 * Extends ListFilter with UI-only state that isn't sent to the daemon.
 * `_timePreset` tracks which named preset is active in the time dropdown so
 * the select can restore its selected value after a re-render.
 */
/** Library type-filter: all recordings, single-track only, or meetings only. */
export type RecordingKind = "all" | "single" | "meeting";

export type UiFilter = ListFilter & {
  semantic?: boolean;
  /** UI-only: filter the list by recording type (client-side, on meeting_id). */
  kind?: RecordingKind;
};

export const filterStore = new Store<UiFilter>({});
