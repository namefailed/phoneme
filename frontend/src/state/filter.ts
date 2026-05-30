import { Store } from "./store";
import type { ListFilter } from "../services/ipc";

/**
 * Extends ListFilter with UI-only state that isn't sent to the daemon.
 * `_timePreset` tracks which named preset is active in the time dropdown so
 * the select can restore its selected value after a re-render.
 */
export type UiFilter = ListFilter;

export const filterStore = new Store<UiFilter>({});
