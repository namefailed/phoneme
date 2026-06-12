// Vitest runs with environment: "jsdom" (vite.config.ts), so document/window
// exist globally and Lit custom elements render for real.
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { DoctorCheckInfo } from "./doctorChecks";

// Stub the CSS import so Vitest doesn't choke on stylesheet syntax.
vi.mock("./modal.css", () => ({}));

const runDoctorMock = vi.fn<[], Promise<DoctorCheckInfo[]>>();
vi.mock("../services/ipc", () => ({
  runDoctor: (...args: unknown[]) => runDoctorMock(...(args as [])),
}));

const invokeMock = vi.fn<unknown[], Promise<unknown>>().mockResolvedValue(undefined);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("../utils/toast", () => ({ showToast: vi.fn() }));

await import("./DoctorModal");

const CHECKS: DoctorCheckInfo[] = [
  {
    name: "Audio directory",
    ok: true,
    detail: "C:/audio (writable)",
    fix_action: "open_audio_dir",
    category: "info",
    explanation: "Verifies the recording folder exists and is writable.",
    fix_hint: null,
  },
  {
    name: "Whisper server",
    ok: false,
    detail: "http://127.0.0.1:5809 — not reachable",
    fix_action: "restart_whisper",
    category: "critical",
    explanation: "Probes the transcription server.",
    fix_hint: "Use Fix to respawn the bundled server.",
  },
  {
    name: "Live-preview server",
    ok: false,
    detail: "http://127.0.0.1:5810 — not reachable",
    fix_action: "restart_whisper",
    category: "warning",
    explanation: "Probes the dedicated live-preview server.",
    fix_hint: "Use Fix to respawn the bundled server(s).",
  },
  {
    name: "Hook command",
    ok: false,
    detail: "missing.exe",
    fix_action: "open_hooks_folder",
    category: "warning",
    explanation: "Verifies the post-transcription hook resolves.",
    fix_hint: "Fix the command path in hook.commands.",
  },
];

function modal() {
  return document.querySelector("ph-doctor-modal") as (HTMLElement & { updateComplete: Promise<unknown> }) | null;
}

function q<T extends HTMLElement>(selector: string): T | null {
  return modal()?.querySelector<T>(selector) ?? null;
}

function qa(selector: string): HTMLElement[] {
  return Array.from(modal()?.querySelectorAll<HTMLElement>(selector) ?? []);
}

async function mount(checks: DoctorCheckInfo[]) {
  runDoctorMock.mockResolvedValue(checks);
  const el = document.createElement("ph-doctor-modal");
  document.body.appendChild(el);
  // Let the initial refresh() resolve and the first render settle.
  await new Promise((r) => setTimeout(r, 0));
  await (el as HTMLElement & { updateComplete: Promise<unknown> }).updateComplete;
}

beforeEach(() => {
  runDoctorMock.mockReset();
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  document.querySelectorAll("ph-doctor-modal").forEach((el) => el.remove());
});

afterEach(() => {
  vi.useRealTimers();
  document.querySelectorAll("ph-doctor-modal").forEach((el) => el.remove());
});

describe("DoctorModal category rendering", () => {
  it("shows a category badge on failing rows only", async () => {
    await mount(CHECKS);
    const badges = qa(".doctor-cat");
    expect(badges.map((b) => b.textContent?.trim())).toEqual(["Critical", "Warning", "Warning"]);
    expect(badges[0].classList.contains("critical")).toBe(true);
    // The passing row renders no badge.
    const okRow = qa(".doctor-row.ok")[0];
    expect(okRow.querySelector(".doctor-cat")).toBeNull();
  });

  it("shows the explanation under every check and the hint only on failures", async () => {
    await mount(CHECKS);
    expect(qa(".doctor-explain")).toHaveLength(4);
    const hints = qa(".doctor-hint");
    expect(hints).toHaveLength(3);
    expect(hints[2].textContent).toContain("hook.commands");
  });
});

describe("DoctorModal Fix All", () => {
  it("is hidden when nothing fixable failed", async () => {
    await mount([CHECKS[0]]);
    expect(q(".doctor-fix-all")).toBeNull();
  });

  it("runs every available fix once, top-down, then re-checks", async () => {
    await mount(CHECKS);
    const btn = q<HTMLButtonElement>(".doctor-fix-all")!;
    // Two distinct actions: restart_whisper (shared by two checks) + hooks.
    expect(btn.textContent).toContain("Fix All (2)");

    vi.useFakeTimers();
    runDoctorMock.mockClear();
    btn.click();
    await modal()!.updateComplete;

    // Disabled while the sweep runs — including the per-row Fix buttons.
    expect(btn.disabled).toBe(true);
    expect(qa(".doctor-fix").every((b) => (b as HTMLButtonElement).disabled)).toBe(true);

    // restart_whisper settles for 5s, open_hooks_folder for 600ms.
    await vi.advanceTimersByTimeAsync(5000);
    await vi.advanceTimersByTimeAsync(600);
    vi.useRealTimers();
    await new Promise((r) => setTimeout(r, 0));

    expect(invokeMock.mock.calls.map((c) => c[0])).toEqual([
      "restart_whisper",
      "open_hooks_folder",
    ]);
    // One re-check at the end of the sweep, not one per action.
    expect(runDoctorMock).toHaveBeenCalledTimes(1);
  });

  it("keeps sweeping when one fix fails", async () => {
    await mount(CHECKS);
    invokeMock.mockImplementation((cmd: unknown) =>
      cmd === "restart_whisper" ? Promise.reject(new Error("nope")) : Promise.resolve(undefined),
    );

    runDoctorMock.mockClear();
    q<HTMLButtonElement>(".doctor-fix-all")!.click();
    // The failed restart skips its settle wait; the hooks fix runs next.
    vi.useFakeTimers();
    await vi.advanceTimersByTimeAsync(600);
    vi.useRealTimers();
    await new Promise((r) => setTimeout(r, 0));

    expect(invokeMock.mock.calls.map((c) => c[0])).toEqual([
      "restart_whisper",
      "open_hooks_folder",
    ]);
    expect(runDoctorMock).toHaveBeenCalledTimes(1);
  });
});
