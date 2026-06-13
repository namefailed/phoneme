/**
 * The app's entire routing layer: which one of the four top-level views is
 * showing. There are no URLs, history, or route params — Tauri serves a single
 * page — so a router here is just a reactive `Store` that `App.mount()`
 * subscribes to, tearing down the old view and constructing the new one on
 * each change.
 */
import { Store } from "./state/store";

/** The four top-level views. "recordings" (the library) is the home view;
 *  "wizard" is the first-run setup, auto-entered when no config exists. */
export type ViewName = "recordings" | "settings" | "doctor" | "wizard";

/** The router's observable state — just the active view. */
export type RouterState = {
  current: ViewName;
};

/**
 * Owns the active-view state. `App` constructs exactly one and subscribes to
 * `state`; anything holding the router can navigate with `go`. Components
 * deeper in the tree don't get a router reference — they dispatch the
 * `phoneme:navigate` window event, which App routes through its unsaved-edits
 * guard before calling `go` (see `App.tryNavigate`).
 */
export class Router {
  /** Observable view state. Subscribe to re-render on navigation. */
  state = new Store<RouterState>({ current: "recordings" });

  /** Switch to `view`. No-op (no notification) if it's already current. */
  go(view: ViewName) {
    this.state.set({ current: view });
  }
}
