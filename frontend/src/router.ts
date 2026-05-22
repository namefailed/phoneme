import { Store } from "./state/store";

export type ViewName = "recordings" | "settings" | "doctor" | "wizard";

export type RouterState = {
  current: ViewName;
};

export class Router {
  state = new Store<RouterState>({ current: "recordings" });

  go(view: ViewName) {
    this.state.set({ current: view });
  }
}
