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

describe("DoctorModal health strip", () => {
  it("shows count chips per failing category", async () => {
    await mount(CHECKS);
    const chips = qa(".doctor-strip .doctor-chip");
    expect(chips.map((c) => c.textContent?.trim())).toEqual(["1 critical", "2 warning"]);
    expect(chips[0].classList.contains("critical")).toBe(true);
    expect(chips[1].classList.contains("warning")).toBe(true);
  });

  it("shows the all-good state when everything passes", async () => {
    await mount([CHECKS[0]]);
    expect(q(".doctor-strip .doctor-chip.ok")?.textContent).toContain("All systems good");
    expect(qa(".doctor-row")).toHaveLength(0);
  });

  it("updates the counts after a re-check", async () => {
    await mount(CHECKS);
    runDoctorMock.mockResolvedValue(CHECKS.map((c) => ({ ...c, ok: true })));
    q<HTMLButtonElement>(".doctor-rerun")!.click();
    await new Promise((r) => setTimeout(r, 0));
    await modal()!.updateComplete;
    expect(q(".doctor-strip .doctor-chip.ok")?.textContent).toContain("All systems good");
    expect(q(".doctor-passing > summary")?.textContent).toContain("4 checks passing");
  });

  it("keeps the current rows on screen while re-running (no layout jump)", async () => {
    await mount(CHECKS);
    let settle!: (v: DoctorCheckInfo[]) => void;
    runDoctorMock.mockImplementation(() => new Promise<DoctorCheckInfo[]>((r) => { settle = r; }));
    q<HTMLButtonElement>(".doctor-rerun")!.click();
    await modal()!.updateComplete;
    // The failing rows stay rendered — no blank-out — with controls disabled.
    expect(qa(".doctor-row")).toHaveLength(3);
    expect(q(".doctor-empty")).toBeNull();
    expect(q<HTMLButtonElement>(".doctor-rerun")!.disabled).toBe(true);
    expect(q<HTMLButtonElement>(".doctor-fix-all")!.disabled).toBe(true);
    settle(CHECKS);
    await new Promise((r) => setTimeout(r, 0));
    await modal()!.updateComplete;
    expect(q<HTMLButtonElement>(".doctor-rerun")!.disabled).toBe(false);
  });

  it("shows the daemon-unreachable state when the check run fails", async () => {
    runDoctorMock.mockRejectedValue(new Error("ipc down"));
    const el = document.createElement("ph-doctor-modal");
    document.body.appendChild(el);
    await new Promise((r) => setTimeout(r, 0));
    await (el as HTMLElement & { updateComplete: Promise<unknown> }).updateComplete;
    expect(q(".doctor-strip-note.err")?.textContent).toContain("Couldn't reach the daemon");
    expect(q(".doctor-empty.err")?.textContent).toContain("ipc down");
  });
});

describe("DoctorModal layout: failures first, passing collapsed", () => {
  it("renders failing checks first as detailed rows, passing behind a collapsed fold", async () => {
    await mount(CHECKS);
    // Only the failures get full rows, in backend order.
    const rows = qa(".doctor-row");
    const names = rows.map((r) => r.querySelector(".doctor-name")?.textContent ?? "");
    expect(names).toHaveLength(3);
    expect(names[0]).toContain("Whisper server");
    expect(names[1]).toContain("Live-preview server");
    expect(names[2]).toContain("Hook command");
    // The passing fold comes after every failing row and starts collapsed.
    const fold = q<HTMLDetailsElement>(".doctor-passing")!;
    expect(fold.open).toBe(false);
    expect(fold.querySelector("summary")?.textContent).toContain("1 check passing");
    for (const row of rows) {
      expect(row.compareDocumentPosition(fold) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
  });

  it("shows a category badge on failing rows only", async () => {
    await mount(CHECKS);
    const badges = qa(".doctor-cat");
    expect(badges.map((b) => b.textContent?.trim())).toEqual(["Critical", "Warning", "Warning"]);
    expect(badges[0].classList.contains("critical")).toBe(true);
    // The passing check renders as a compact row with no badge.
    expect(q(".doctor-pass-row .doctor-cat")).toBeNull();
  });

  it("shows the explanation and hint on failing rows; passing rows stay one-line", async () => {
    await mount(CHECKS);
    expect(qa(".doctor-explain")).toHaveLength(3);
    const hints = qa(".doctor-hint");
    expect(hints).toHaveLength(3);
    expect(hints[2].textContent).toContain("hook.commands");
    // Compact passing row: name + detail, explanation tucked into the tooltip.
    const passRow = q(".doctor-pass-row")!;
    expect(passRow.querySelector(".doctor-pass-name")?.textContent).toBe("Audio directory");
    expect(passRow.querySelector(".doctor-pass-detail")?.textContent).toContain("C:/audio");
    expect(passRow.getAttribute("title")).toContain("recording folder");
    expect(passRow.querySelector(".doctor-explain")).toBeNull();
  });

  it("groups expanded passing checks by subsystem with a fallback header", async () => {
    await mount([
      { name: "Whisper server", ok: true, detail: "HTTP 200" },
      { name: "Audio directory", ok: true, detail: "writable" },
      { name: "Mystery probe", ok: true, detail: "fine" },
    ]);
    expect(qa(".doctor-group-title").map((t) => t.textContent?.trim())).toEqual([
      "Servers",
      "Storage",
      "Other",
    ]);
    expect(qa(".doctor-pass-name").map((n) => n.textContent)).toEqual([
      "Whisper server",
      "Audio directory",
      "Mystery probe",
    ]);

    // Per-group MEMBERSHIP, not just overall order: each passing check must
    // render UNDER its own subsystem header inside its own .doctor-group, so a
    // mis-bucketing that kept the flat order would still be caught here.
    const groups = qa(".doctor-group").map((g) => ({
      title: g.querySelector(".doctor-group-title")?.textContent?.trim(),
      names: Array.from(g.querySelectorAll(".doctor-pass-name")).map((n) => n.textContent),
    }));
    expect(groups).toEqual([
      { title: "Servers", names: ["Whisper server"] },
      { title: "Storage", names: ["Audio directory"] },
      { title: "Other", names: ["Mystery probe"] },
    ]);
  });

  it("toggles the passing fold via its summary", async () => {
    await mount(CHECKS);
    const fold = q<HTMLDetailsElement>(".doctor-passing")!;
    const sum = fold.querySelector("summary")!;
    expect(fold.open).toBe(false);
    sum.click();
    expect(fold.open).toBe(true);
    sum.click();
    expect(fold.open).toBe(false);
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
