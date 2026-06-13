// Vitest runs with environment: "jsdom" (vite.config.ts), so document/window
// exist globally and Lit custom elements render for real. This suite pins the
// health pill's polling contract: the 30s Doctor poll runs only while the
// window is visible, a poll that comes due while hidden runs on re-show, and
// everything unhooks on disconnect.
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

const runDoctorMock = vi.fn(async () => [] as unknown[]);
vi.mock("../services/ipc", () => ({
  listTags: vi.fn(async () => []),
  runDoctor: (...args: unknown[]) => runDoctorMock(...(args as [])),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({})),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

vi.mock("../utils/toast", () => ({ showToast: vi.fn() }));

await import("./HeaderBar");

/** Shadow jsdom's prototype getter so the component sees this visibility. */
function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    get: () => state,
  });
}

let el: HTMLElement;

beforeEach(async () => {
  vi.useFakeTimers();
  runDoctorMock.mockClear();
  setVisibility("visible");
  el = document.createElement("ph-header-bar");
  document.body.appendChild(el);
  // Let connectedCallback's async setup (mocked listen/invoke chains) settle.
  await vi.advanceTimersByTimeAsync(0);
});

afterEach(() => {
  el.remove();
  Reflect.deleteProperty(document, "visibilityState"); // back to the prototype getter
  vi.useRealTimers();
});

describe("health polling visibility gate", () => {
  it("checks once on mount and again every 30s while visible", async () => {
    expect(runDoctorMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(runDoctorMock).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(runDoctorMock).toHaveBeenCalledTimes(3);
  });

  it("skips polls while hidden, then runs the deferred check on re-show", async () => {
    expect(runDoctorMock).toHaveBeenCalledTimes(1);

    setVisibility("hidden");
    // Three intervals elapse in the tray — none may probe the backends.
    await vi.advanceTimersByTimeAsync(90_000);
    expect(runDoctorMock).toHaveBeenCalledTimes(1);

    // The window comes back: the deferred check runs immediately...
    setVisibility("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(runDoctorMock).toHaveBeenCalledTimes(2);

    // ...and the regular cadence resumes.
    await vi.advanceTimersByTimeAsync(30_000);
    expect(runDoctorMock).toHaveBeenCalledTimes(3);
  });

  it("becoming visible with no deferred check does not double-poll", async () => {
    expect(runDoctorMock).toHaveBeenCalledTimes(1);
    // No hidden interval elapsed — a bare visibilitychange is a no-op.
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(runDoctorMock).toHaveBeenCalledTimes(1);
  });

  it("disconnect stops both the interval and the visibility listener", async () => {
    setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(30_000); // marks a check as due
    el.remove();

    setVisibility("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(120_000);
    expect(runDoctorMock).toHaveBeenCalledTimes(1); // only the mount check
  });
});
