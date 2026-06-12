import { describe, it, expect } from 'vitest';
import { filterStore, applyMoreLikeThis, clearMoreLikeThis } from './filter';

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

describe('More-like-this filter helpers', () => {
  it('applyMoreLikeThis enters like-mode and clears the text search', () => {
    filterStore.set({ search: 'typed query', tag_id: 3 });
    applyMoreLikeThis('20260519T143500823', 'Standup notes');
    expect(filterStore.get()).toEqual({
      search: null,
      tag_id: 3, // other dimensions are left alone
      like_id: '20260519T143500823',
      like_label: 'Standup notes',
    });
  });

  it('applyMoreLikeThis stores no label for a blank title', () => {
    applyMoreLikeThis('20260519T143500823', '   ');
    expect(filterStore.get().like_label).toBeNull();
  });

  it('clearMoreLikeThis returns to the normal list', () => {
    applyMoreLikeThis('20260519T143500823', 'Standup notes');
    clearMoreLikeThis();
    const f = filterStore.get();
    expect(f.like_id).toBeNull();
    expect(f.like_label).toBeNull();
  });
});
