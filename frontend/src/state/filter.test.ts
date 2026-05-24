import { describe, it, expect } from 'vitest';
import { filterStore } from './filter';

describe('Filter Store', () => {
  it('initializes as an empty object', () => {
    expect(filterStore.get()).toEqual({});
  });

  it('can be updated with filters', () => {
    filterStore.set({ status: 'recording', search: 'hello' });
    expect(filterStore.get()).toEqual({ status: 'recording', search: 'hello' });
  });

  it('retains previous properties when spread', () => {
    filterStore.set({ ...filterStore.get(), limit: 10 });
    expect(filterStore.get()).toEqual({ status: 'recording', search: 'hello', limit: 10 });
  });
});
