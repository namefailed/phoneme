// The app-health polling contract used to live on the header bar; it now lives in
// this shared store (so the pill stays live in Settings, where the bar is hidden).
// This suite pins that contract: one poll on start, every 30s while visible, no
// probing while hidden, the deferred check runs on re-show, and start is idempotent
// (a single poll no matter how many consumers call it). jsdom env (vite.config.ts).
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

const runDoctorMock = vi.fn(async () => [] as unknown[]);
vi.mock("../services/ipc", () => ({
  runDoctor: (...args: unknown[]) => runDoctorMock(...(args as [])),
}));

/** Shadow jsdom's prototype getter so the store sees this visibility. */
function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    get: () => state,
  });
}

// The store is a module singleton (one poll for the whole app), so the full
// lifecycle is exercised in a single sequential test rather than across isolated
// cases — re-importing for a "fresh" singleton would leak its document listener.
let startHealthPolling: () => void;

beforeEach(async () => {
  vi.useFakeTimers();
  runDoctorMock.mockClear();
  setVisibility("visible");
  ({ startHealthPolling } = await import("./health"));
});

afterEach(() => {
  Reflect.deleteProperty(document, "visibilityState"); // back to the prototype getter
  vi.useRealTimers();
});

describe("shared health poll", () => {
  it("polls once on start, every 30s while visible, defers while hidden, and is idempotent", async () => {
    startHealthPolling();
    await vi.advanceTimersByTimeAsync(0);
    expect(runDoctorMock).toHaveBeenCalledTimes(1); // the initial check

    // A second consumer calling start must NOT spin up a second poll.
    startHealthPolling();
    await vi.advanceTimersByTimeAsync(0);
    expect(runDoctorMock).toHaveBeenCalledTimes(1);

    // Regular 30s cadence while visible.
    await vi.advanceTimersByTimeAsync(30_000);
    expect(runDoctorMock).toHaveBeenCalledTimes(2);

    // Minimized / tray: three intervals elapse, none may probe the backends.
    setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(90_000);
    expect(runDoctorMock).toHaveBeenCalledTimes(2);

    // The window comes back: the deferred check runs immediately...
    setVisibility("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(runDoctorMock).toHaveBeenCalledTimes(3);

    // ...and the regular cadence resumes.
    await vi.advanceTimersByTimeAsync(30_000);
    expect(runDoctorMock).toHaveBeenCalledTimes(4);

    // A bare visibilitychange with nothing deferred is a no-op (no double-poll).
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(runDoctorMock).toHaveBeenCalledTimes(4);
  });
});
