import { describe, it, expect, vi } from 'vitest';
import { Store } from './store';

describe('Store', () => {
  it('initializes with the correct value', () => {
    const store = new Store(10);
    expect(store.get()).toBe(10);
  });

  it('updates value and notifies subscribers', () => {
    const store = new Store(0);
    const sub = vi.fn();
    
    // Subscriber gets called immediately on subscribe
    store.subscribe(sub);
    expect(sub).toHaveBeenCalledWith(0);
    expect(sub).toHaveBeenCalledTimes(1);

    store.set(5);
    expect(store.get()).toBe(5);
    expect(sub).toHaveBeenCalledWith(5);
    expect(sub).toHaveBeenCalledTimes(2);
  });

  it('updates value using a function', () => {
    const store = new Store(10);
    store.set((prev) => prev + 5);
    expect(store.get()).toBe(15);
  });

  it('does not notify if value is identical', () => {
    const store = new Store('test');
    const sub = vi.fn();
    store.subscribe(sub);
    expect(sub).toHaveBeenCalledTimes(1);

    store.set('test');
    expect(sub).toHaveBeenCalledTimes(1);
  });

  it('change detection is by reference identity, not value equality', () => {
    // The contract is === identity. A primitive can't distinguish identity from
    // value equality, so use objects: a new, value-equal object DOES notify
    // (different ref); re-setting the same reference does NOT. This catches a
    // regression from identity to deep/value equality, which the primitive
    // case above cannot.
    const initial = { a: 1 };
    const store = new Store(initial);
    const sub = vi.fn();
    store.subscribe(sub);
    expect(sub).toHaveBeenCalledTimes(1); // immediate call on subscribe

    // A value-equal but reference-different object: still a change → notifies.
    const replacement = { a: 1 };
    store.set(replacement);
    expect(sub).toHaveBeenCalledTimes(2);
    expect(sub).toHaveBeenLastCalledWith(replacement);
    expect(store.get()).toBe(replacement);

    // Re-setting the SAME reference is a no-op — no extra notification.
    store.set(replacement);
    expect(sub).toHaveBeenCalledTimes(2);

    // And the original reference is likewise treated as a fresh change.
    store.set(initial);
    expect(sub).toHaveBeenCalledTimes(3);
    expect(sub).toHaveBeenLastCalledWith(initial);
  });

  it('can unsubscribe', () => {
    const store = new Store(0);
    const sub = vi.fn();
    const unsub = store.subscribe(sub);
    
    unsub();
    store.set(10);
    
    // Should only have been called once during initial subscribe
    expect(sub).toHaveBeenCalledTimes(1);
  });
});
