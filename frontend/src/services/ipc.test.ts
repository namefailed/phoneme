import { describe, it, expect, vi } from 'vitest';
import { listRecordings, recordStart } from './ipc';
import * as tauriCore from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => {
  return {
    invoke: vi.fn(),
  };
});

describe('IPC Services', () => {
  it('calls list_recordings with correct filter', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce([]);
    
    const filter = { status: 'done' };
    const res = await listRecordings(filter);
    
    expect(tauriCore.invoke).toHaveBeenCalledWith('list_recordings', { filter });
    expect(res).toEqual([]);
  });

  it('calls record_start with correct mode', async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValueOnce({ id: '123' });
    
    const res = await recordStart('oneshot');
    
    expect(tauriCore.invoke).toHaveBeenCalledWith('record_start', { mode: 'oneshot' });
    expect(res).toEqual({ id: '123' });
  });
});
