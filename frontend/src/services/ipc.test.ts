import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  listRecordings,
  listSession,
  recordStart,
  deleteRecording,
  moreLikeThis,
  exportCaptions,
  exportLibraryZip,
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

  it('deleteRecording defaults to removing the audio too (keepAudio: false)', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(undefined);

    await deleteRecording('20260519T143500823');

    expect(tauriCore.invoke).toHaveBeenCalledWith('delete_recording', {
      id: '20260519T143500823',
      keepAudio: false,
    });
  });

  it('deleteRecording passes keepAudio: true for a keep-audio delete', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(undefined);

    await deleteRecording('20260519T143500823', true);

    expect(tauriCore.invoke).toHaveBeenCalledWith('delete_recording', {
      id: '20260519T143500823',
      keepAudio: true,
    });
  });

  it('moreLikeThis pins the more_like_this payload (id + default limit)', async () => {
    const results = [{ recording: { id: '20260519T150000000' }, score: 0.8 }];
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(results);

    const res = await moreLikeThis('20260519T143500823');

    expect(tauriCore.invoke).toHaveBeenCalledWith('more_like_this', {
      id: '20260519T143500823',
      limit: 20,
    });
    expect(res).toEqual(results);
  });

  it('moreLikeThis passes a custom limit through', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce([]);

    await moreLikeThis('20260519T143500823', 5);

    expect(tauriCore.invoke).toHaveBeenCalledWith('more_like_this', {
      id: '20260519T143500823',
      limit: 5,
    });
  });

  it('exportCaptions forwards id + format and returns the caption body', async () => {
    const body = '1\n00:00:01,000 --> 00:00:04,500\nHello world.\n';
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(body);

    const res = await exportCaptions('20260519T143500823', 'srt');

    expect(tauriCore.invoke).toHaveBeenCalledWith('export_captions', {
      id: '20260519T143500823',
      format: 'srt',
    });
    expect(res).toBe(body);
  });

  it('exportCaptions passes the vtt format through unchanged', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce('WEBVTT\n\n');

    await exportCaptions('20260519T143500823', 'vtt');

    expect(tauriCore.invoke).toHaveBeenCalledWith('export_captions', {
      id: '20260519T143500823',
      format: 'vtt',
    });
  });

  it('exportLibraryZip forwards the dest path and returns the audio count', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce(3);

    const count = await exportLibraryZip('C:\\backups\\phoneme-backup.zip');

    expect(tauriCore.invoke).toHaveBeenCalledWith('export_library_zip', {
      dest: 'C:\\backups\\phoneme-backup.zip',
    });
    expect(count).toBe(3);
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
