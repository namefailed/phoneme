/**
 * Tiny reactive store — observable state without a framework. This is the
 * app's entire state-management machinery: a current value plus a set of
 * subscriber callbacks, nothing more (no reducers, actions, or middleware).
 *
 * Instances are created where the state lives: module-level singletons for
 * cross-cutting state (`filterStore`, the router's view state) and per-view
 * instances for local state (RecordingsView's list state). Lit components
 * subscribe in `connectedCallback` and unsubscribe in `disconnectedCallback`;
 * plain classes subscribe in their constructor and unsubscribe in `dispose`.
 */

/** A change callback. Receives the new value on every `set` that changes it. */
export type Subscriber<T> = (value: T) => void;

/**
 * An observable value. Change detection is by identity (`===`), so state has to
 * be updated immutably — `set({ ...store.get(), field: x })`, never mutation
 * of the held object (a mutated object compares equal to itself and notifies
 * nobody). Subscribers run synchronously, in insertion order, inside `set`.
 */
export class Store<T> {
  private value: T;
  private subscribers = new Set<Subscriber<T>>();

  constructor(initial: T) {
    this.value = initial;
  }

  /** The current value (the live reference — treat it as read-only). */
  get(): T {
    return this.value;
  }

  /**
   * Replace the value and notify all subscribers. Accepts either the next
   * value or an updater `(prev) => next`. A `===`-identical result is a
   * no-op: nothing is stored and nobody is notified.
   */
  set(updater: T | ((prev: T) => T)): void {
    const next =
      typeof updater === "function" ? (updater as (prev: T) => T)(this.value) : updater;
    if (next === this.value) return;
    this.value = next;
    for (const sub of this.subscribers) sub(next);
  }

  /**
   * Register `sub` and immediately invoke it with the current value (so a
   * fresh subscriber renders without waiting for the next change). Returns
   * the unsubscribe function; every subscriber has to call it on teardown or
   * the store keeps the callback (and whatever it closes over) alive.
   */
  subscribe(sub: Subscriber<T>): () => void {
    this.subscribers.add(sub);
    sub(this.value);
    return () => {
      this.subscribers.delete(sub);
    };
  }
}
