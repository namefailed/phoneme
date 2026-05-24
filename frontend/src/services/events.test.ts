import { describe, it, expect, vi } from 'vitest';
import { subscribe, onMenu, onNav, type DaemonEvent } from './events';
import * as tauriEvent from '@tauri-apps/api/event';

vi.mock('@tauri-apps/api/event', () => {
  return {
    listen: vi.fn(),
  };
});

describe('Event Services', () => {
  it('subscribes to daemon-event and unwraps payload', async () => {
    // Mock the listen function to capture the handler
    let capturedHandler: (e: any) => void = () => {};
    vi.mocked(tauriEvent.listen).mockImplementationOnce(async (event, handler) => {
      expect(event).toBe('daemon-event');
      capturedHandler = handler as (e: any) => void;
      return vi.fn(); // Return mock unlisten fn
    });

    const mockCallback = vi.fn();
    const unsub = await subscribe(mockCallback);
    
    // Simulate an event arriving from Tauri
    const fakeEventPayload: DaemonEvent = { event: 'recording_started', id: '123', started_at: 'now' };
    capturedHandler({ payload: fakeEventPayload });
    
    expect(mockCallback).toHaveBeenCalledWith(fakeEventPayload);
    expect(unsub).toBeTypeOf('function');
  });

  it('subscribes to onMenu events', async () => {
    vi.mocked(tauriEvent.listen).mockResolvedValueOnce(vi.fn());
    
    const mockCallback = vi.fn();
    await onMenu('settings', mockCallback);
    
    expect(tauriEvent.listen).toHaveBeenCalledWith('menu:settings', expect.any(Function));
  });

  it('subscribes to onNav events', async () => {
    vi.mocked(tauriEvent.listen).mockResolvedValueOnce(vi.fn());
    
    const mockCallback = vi.fn();
    await onNav('wizard', mockCallback);
    
    expect(tauriEvent.listen).toHaveBeenCalledWith('nav:wizard', expect.any(Function));
  });
});
