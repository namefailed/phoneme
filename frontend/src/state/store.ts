// Tiny reactive store — observable state without a framework.

export type Subscriber<T> = (value: T) => void;

export class Store<T> {
  private value: T;
  private subscribers = new Set<Subscriber<T>>();

  constructor(initial: T) {
    this.value = initial;
  }

  get(): T {
    return this.value;
  }

  set(updater: T | ((prev: T) => T)): void {
    const next =
      typeof updater === "function" ? (updater as (prev: T) => T)(this.value) : updater;
    if (next === this.value) return;
    this.value = next;
    for (const sub of this.subscribers) sub(next);
  }

  subscribe(sub: Subscriber<T>): () => void {
    this.subscribers.add(sub);
    sub(this.value);
    return () => {
      this.subscribers.delete(sub);
    };
  }
}
