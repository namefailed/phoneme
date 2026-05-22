import { Store } from "./store";
import type { ListFilter } from "../services/ipc";

export const filterStore = new Store<ListFilter>({});
