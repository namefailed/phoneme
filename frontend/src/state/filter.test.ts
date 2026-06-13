import { describe, it, expect } from 'vitest';
import { filterStore, applyMoreLikeThis, clearMoreLikeThis, toWireFilter } from './filter';

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

describe('toWireFilter', () => {
  it('passes the daemon-side fields through and drops UI-only state', () => {
    const wire = toWireFilter({
      search: 'standup',
      tag_id: 4,
      limit: 100,
      offset: 200,
      sort_desc: false,
      semantic: true,            // UI-only
      like_id: '20260519T1435',  // UI-only
      like_label: 'Notes',       // UI-only
    });
    expect(wire).toEqual({
      limit: 100,
      offset: 200,
      since: undefined,
      until: undefined,
      status: undefined,
      search: 'standup',
      tag_id: 4,
      sort_desc: false,
    });
    expect('semantic' in wire).toBe(false);
    expect('like_id' in wire).toBe(false);
  });

  it('maps kind single/meeting onto the wire kind field', () => {
    expect(toWireFilter({ kind: 'single' }).kind).toBe('single');
    expect(toWireFilter({ kind: 'meeting' }).kind).toBe('meeting');
    expect(toWireFilter({ kind: 'single' }).favorite).toBeUndefined();
  });

  it('maps the favorite kind onto the wire favorite flag, not kind', () => {
    const wire = toWireFilter({ kind: 'favorite' });
    expect(wire.favorite).toBe(true);
    expect(wire.kind).toBeUndefined();
  });

  it('sends neither field for "all" or an unset kind', () => {
    for (const f of [toWireFilter({ kind: 'all' }), toWireFilter({})]) {
      expect(f.kind).toBeUndefined();
      expect(f.favorite).toBeUndefined();
    }
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
