import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  listRecordings,
  listSession,
  recordStart,
  listTags,
  listAllTags,
  addTag,
  updateTag,
  deleteTag,
  attachTag,
  detachTag,
  tagsFor,
  type Tag,
} from './ipc';
import * as tauriCore from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => {
  return {
    invoke: vi.fn(),
  };
});

beforeEach(() => {
  vi.mocked(tauriCore.invoke).mockReset();
});

describe('IPC Services', () => {
  it('calls list_recordings with correct filter', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce([]);

    const filter = { status: 'done' };
    const res = await listRecordings(filter);

    expect(tauriCore.invoke).toHaveBeenCalledWith('list_recordings', { filter });
    expect(res).toEqual([]);
  });

  it('calls list_meeting with the camelCase meetingId arg', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce([]);

    const res = await listSession('sess-1');

    expect(tauriCore.invoke).toHaveBeenCalledWith('list_meeting', { meetingId: 'sess-1' });
    expect(res).toEqual([]);
  });

  it('calls record_start with correct mode', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce({ id: '123' });

    const res = await recordStart('oneshot');

    expect(tauriCore.invoke).toHaveBeenCalledWith('record_start', { mode: 'oneshot' });
    expect(res).toEqual({ id: '123' });
  });
});

describe('Tag IPC functions', () => {
  const fakeTags: Tag[] = [
    { id: 1, name: 'work', color: '#f38ba8' },
    { id: 2, name: 'personal', color: null },
  ];

  it('listTags calls list_tags and returns the result', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(fakeTags);
    const result = await listTags();
    expect(tauriCore.invoke).toHaveBeenCalledWith('list_tags');
    expect(result).toEqual(fakeTags);
  });

  it('listAllTags calls list_all_tags and returns the result', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(fakeTags);
    const result = await listAllTags();
    expect(tauriCore.invoke).toHaveBeenCalledWith('list_all_tags');
    expect(result).toEqual(fakeTags);
  });

  it('addTag calls add_tag with name and color', async () => {
    const newTag: Tag = { id: 3, name: 'music', color: '#cba6f7' };
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(newTag);
    const result = await addTag('music', '#cba6f7');
    expect(tauriCore.invoke).toHaveBeenCalledWith('add_tag', { name: 'music', color: '#cba6f7' });
    expect(result).toEqual(newTag);
  });

  it('addTag sends color: null when no color is provided', async () => {
    const newTag: Tag = { id: 4, name: 'uncolored', color: null };
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(newTag);
    await addTag('uncolored');
    expect(tauriCore.invoke).toHaveBeenCalledWith('add_tag', { name: 'uncolored', color: null });
  });

  it('updateTag calls update_tag with id, name, and color', async () => {
    const updated: Tag = { id: 1, name: 'renamed', color: '#a6e3a1' };
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(updated);
    const result = await updateTag(1, 'renamed', '#a6e3a1');
    expect(tauriCore.invoke).toHaveBeenCalledWith('update_tag', { id: 1, name: 'renamed', color: '#a6e3a1' });
    expect(result).toEqual(updated);
  });

  it('updateTag sends color: null when color is explicitly cleared', async () => {
    const updated: Tag = { id: 1, name: 'work', color: null };
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(updated);
    await updateTag(1, 'work', null);
    expect(tauriCore.invoke).toHaveBeenCalledWith('update_tag', { id: 1, name: 'work', color: null });
  });

  it('deleteTag calls delete_tag with the id', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(undefined);
    await deleteTag(5);
    expect(tauriCore.invoke).toHaveBeenCalledWith('delete_tag', { id: 5 });
  });

  it('attachTag calls attach_tag with recordingId and tagId', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(undefined);
    await attachTag('rec-abc', 2);
    expect(tauriCore.invoke).toHaveBeenCalledWith('attach_tag', { recordingId: 'rec-abc', tagId: 2 });
  });

  it('detachTag calls detach_tag with recordingId and tagId', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(undefined);
    await detachTag('rec-abc', 2);
    expect(tauriCore.invoke).toHaveBeenCalledWith('detach_tag', { recordingId: 'rec-abc', tagId: 2 });
  });

  it('tagsFor calls tags_for with recordingId and returns the tags', async () => {
    const tags: Tag[] = [{ id: 1, name: 'work', color: '#f38ba8' }];
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(tags);
    const result = await tagsFor('rec-xyz');
    expect(tauriCore.invoke).toHaveBeenCalledWith('tags_for', { recordingId: 'rec-xyz' });
    expect(result).toEqual(tags);
  });
});
