import { describe, it, expect } from 'vitest';
import {
  filterStore,
  applyMoreLikeThis,
  clearMoreLikeThis,
  applyEntityFilter,
  clearEntityFilter,
  applyTaskFilter,
  clearTaskFilter,
  toWireFilter,
} from './filter';

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

  it('maps the pinned kind onto the wire pinned flag, not kind', () => {
    const wire = toWireFilter({ kind: 'pinned' });
    expect(wire.pinned).toBe(true);
    expect(wire.kind).toBeUndefined();
    expect(wire.favorite).toBeUndefined();
  });

  it('sends neither field for "all" or an unset kind', () => {
    for (const f of [toWireFilter({ kind: 'all' }), toWireFilter({})]) {
      expect(f.kind).toBeUndefined();
      expect(f.favorite).toBeUndefined();
    }
  });

  it('maps the tag-presence state onto the wire tagged flag', () => {
    expect(toWireFilter({ tagState: 'tagged' }).tagged).toBe(true);
    expect(toWireFilter({ tagState: 'untagged' }).tagged).toBe(false);
  });

  it('omits the tagged flag when no tag-presence state is set', () => {
    for (const f of [toWireFilter({ tagState: null }), toWireFilter({})]) {
      expect(f.tagged).toBeUndefined();
      expect('tagged' in f).toBe(false);
    }
  });

  it('maps the entity facet value + kind onto the wire entity fields', () => {
    const wire = toWireFilter({ entity_value: 'Alice', entity_kind: 'person', entity_label: 'Person' });
    expect(wire.entity_value).toBe('Alice');
    expect(wire.entity_kind).toBe('person');
    // The UI-only label never crosses the wire.
    expect('entity_label' in wire).toBe(false);
  });

  it('sends the entity value alone when no kind is set (matches across kinds)', () => {
    const wire = toWireFilter({ entity_value: 'Mercury' });
    expect(wire.entity_value).toBe('Mercury');
    expect('entity_kind' in wire).toBe(false);
  });

  it('omits the entity fields when no entity value is set', () => {
    for (const f of [toWireFilter({ entity_kind: 'person' }), toWireFilter({})]) {
      expect('entity_value' in f).toBe(false);
      expect('entity_kind' in f).toBe(false);
    }
  });

  it('maps the task_state token onto the wire field', () => {
    expect(toWireFilter({ task_state: 'has_open' }).task_state).toBe('has_open');
    expect(toWireFilter({ task_state: 'has_tasks' }).task_state).toBe('has_tasks');
  });

  it('omits task_state when no task filter is set', () => {
    for (const f of [toWireFilter({ task_state: null }), toWireFilter({})]) {
      expect('task_state' in f).toBe(false);
    }
  });
});

describe('Task-filter helpers', () => {
  it('applyTaskFilter sets the state and keeps other dimensions', () => {
    filterStore.set({ search: 'standup', tag_id: 7 });
    applyTaskFilter('has_open');
    expect(filterStore.get()).toEqual({
      search: 'standup',
      tag_id: 7, // it COMBINES — other dimensions untouched
      task_state: 'has_open',
    });
  });

  it('applyTaskFilter on the active state toggles it off', () => {
    filterStore.set({});
    applyTaskFilter('has_open');
    applyTaskFilter('has_open');
    expect(filterStore.get().task_state).toBeNull();
  });

  it('applyTaskFilter to a different state replaces, not toggles', () => {
    filterStore.set({});
    applyTaskFilter('has_open');
    applyTaskFilter('has_tasks');
    expect(filterStore.get().task_state).toBe('has_tasks');
  });

  it('clearTaskFilter returns to the normal list', () => {
    applyTaskFilter('has_tasks');
    clearTaskFilter();
    expect(filterStore.get().task_state).toBeNull();
  });
});

describe('Entity-filter helpers', () => {
  it('applyEntityFilter narrows to one entity and keeps other dimensions', () => {
    filterStore.set({ search: 'typed query', tag_id: 7 });
    applyEntityFilter('Alice', 'person', 'Person');
    expect(filterStore.get()).toEqual({
      search: 'typed query',
      tag_id: 7, // other dimensions are left alone — it COMBINES
      entity_value: 'Alice',
      entity_kind: 'person',
      entity_label: 'Person',
    });
  });

  it('applyEntityFilter on the active entity toggles it off', () => {
    filterStore.set({});
    applyEntityFilter('Alice', 'person', 'Person');
    // Re-clicking the same (value, kind) clears it back to the unfiltered list.
    applyEntityFilter('Alice', 'person', 'Person');
    const f = filterStore.get();
    expect(f.entity_value).toBeNull();
    expect(f.entity_kind).toBeNull();
    expect(f.entity_label).toBeNull();
  });

  it('applyEntityFilter to a different entity replaces, not toggles', () => {
    filterStore.set({});
    applyEntityFilter('Alice', 'person');
    applyEntityFilter('Bob', 'person');
    expect(filterStore.get().entity_value).toBe('Bob');
  });

  it('clearEntityFilter returns to the normal list', () => {
    applyEntityFilter('Alice', 'person', 'Person');
    clearEntityFilter();
    const f = filterStore.get();
    expect(f.entity_value).toBeNull();
    expect(f.entity_kind).toBeNull();
    expect(f.entity_label).toBeNull();
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
