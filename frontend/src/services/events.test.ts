import { describe, it, expect, vi } from 'vitest';
import { subscribe, onMenu, onNav, type DaemonEvent } from './events';
import * as tauriEvent from '@tauri-apps/api/event';

vi.mock('@tauri-apps/api/event', () => {
  return {
    listen: vi.fn(),
  };
});

function captureSubscribeHandler(): Promise<{ handler: (e: any) => void; unsub: () => void }> {
  return new Promise((resolve) => {
    vi.mocked(tauriEvent.listen).mockImplementationOnce(async (_event: string, handler: (e: any) => void) => {
      const unsub = vi.fn();
      resolve({ handler: handler as (e: any) => void, unsub });
      return unsub;
    });
  });
}

describe('Event Services', () => {
  it('subscribes to daemon-event and unwraps payload', async () => {
    const captured = captureSubscribeHandler();
    const mockCallback = vi.fn();
    const unsub = await subscribe(mockCallback);
    const { handler } = await captured;

    const payload: DaemonEvent = { event: 'recording_started', id: '123', started_at: 'now' };
    handler({ payload });

    expect(mockCallback).toHaveBeenCalledWith(payload);
    expect(unsub).toBeTypeOf('function');
  });

  it('forwards every event payload unchanged regardless of variant (no discrimination in subscribe)', async () => {
    // subscribe()'s only logic is `(e) => handler(e.payload)` — it does NOT
    // switch on event.event. One variant-agnostic test pins that contract: a
    // representative set of distinct payloads each arrives byte-for-byte (same
    // reference, by toBe), in order. Per-variant routing is exercised where it
    // actually lives — notifications.test.ts switches on event.event.
    const captured = captureSubscribeHandler();
    const mockCallback = vi.fn();
    await subscribe(mockCallback);
    const { handler } = await captured;

    const payloads: DaemonEvent[] = [
      { event: 'transcription_partial', id: 'abc', text: 'hello wor' },
      { event: 'tag_created', id: 7 },
      { event: 'tag_updated', id: 42 },
      { event: 'tag_deleted', id: 3 },
      { event: 'tag_attached', tag_id: 1 },
      { event: 'tag_detached', tag_id: 1 },
    ];
    for (const payload of payloads) handler({ payload });

    expect(mockCallback).toHaveBeenCalledTimes(payloads.length);
    payloads.forEach((payload, i) => {
      // Reference-identity: the unwrapped payload is forwarded as-is, untouched.
      expect(mockCallback.mock.calls[i][0]).toBe(payload);
    });
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
