// Vitest runs with environment: "jsdom" (vite.config.ts), so document/window
// exist globally and Lit custom elements render for real.
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { Recording, ListFilter } from "../../services/ipc";
import type { DaemonEvent } from "../../services/events";

// Stub the CSS import so Vitest doesn't choke on stylesheet syntax.
vi.mock("../modal.css", () => ({}));

// One ipc mock covers BOTH elements under test: the failed panel and the
// queue panel that opens it (the queue panel pulls in the whole queue API).
const listRecordingsMock = vi.fn<[ListFilter?], Promise<Recording[]>>();
const retranscribeMock = vi.fn<unknown[], Promise<void>>();
const clearFailedMock = vi.fn<[], Promise<number>>();
const getQueueCountsMock = vi.fn<[], Promise<{ pending: number; processing: number; done: number; failed: number }>>();
const listQueueMock = vi.fn<[], Promise<unknown[]>>().mockResolvedValue([]);
const queuePausedMock = vi.fn<[], Promise<boolean>>().mockResolvedValue(false);
vi.mock("../../services/ipc", () => ({
  listRecordings: (...args: [ListFilter?]) => listRecordingsMock(...args),
  retranscribeRecording: (...args: unknown[]) => retranscribeMock(...args),
  clearFailed: () => clearFailedMock(),
  getQueueCounts: () => getQueueCountsMock(),
  listQueue: () => listQueueMock(),
  queuePaused: () => queuePausedMock(),
  cancelQueued: vi.fn(),
  reorderQueue: vi.fn(),
  setQueuePaused: vi.fn(),
  cancelAllQueued: vi.fn(),
  cancelProcessing: vi.fn(),
  getRecording: vi.fn(),
  skipCurrentStage: vi.fn(),
}));

// Capture every subscriber so tests can push daemon events into the panels.
const handlers: Array<(e: DaemonEvent) => void> = [];
vi.mock("../../services/events", () => ({
  subscribe: (h: (e: DaemonEvent) => void) => {
    handlers.push(h);
    return Promise.resolve(() => {
      const i = handlers.indexOf(h);
      if (i >= 0) handlers.splice(i, 1);
    });
  },
  stageLabel: (s: string) => s,
}));

vi.mock("../../utils/toast", () => ({ showToast: vi.fn() }));

const { openFailedPanel, recordFailureDetail } = await import("./FailedPanel");
await import("./QueuePanel");

/** A minimal failed recording row; overrides shape the scenario. */
function rec(id: string, overrides: Partial<Recording> = {}): Recording {
  return {
    id,
    started_at: "2026-06-12T10:00:00Z",
    duration_ms: 65_000,
    audio_path: `C:/audio/${id}.wav`,
    transcript: null,
    model: "base",
    status: "transcribe_failed",
    ...overrides,
  };
}

/** What listRecordings returns per status filter (the panel queries each). */
let byStatus: Record<string, Recording[]>;

function panel() {
  return document.querySelector("ph-failed-panel") as (HTMLElement & { updateComplete: Promise<unknown> }) | null;
}

function q<T extends HTMLElement>(selector: string): T | null {
  return panel()?.querySelector<T>(selector) ?? null;
}

function qa(selector: string): HTMLElement[] {
  return Array.from(panel()?.querySelectorAll<HTMLElement>(selector) ?? []);
}

async function tick() {
  await new Promise((r) => setTimeout(r, 0));
  await panel()?.updateComplete;
}

async function mountPanel() {
  const el = document.createElement("ph-failed-panel");
  document.body.appendChild(el);
  await tick();
}

function emit(e: DaemonEvent) {
  [...handlers].forEach((h) => h(e));
}

beforeEach(() => {
  byStatus = { transcribe_failed: [], hook_failed: [] };
  listRecordingsMock.mockReset();
  listRecordingsMock.mockImplementation((f?: ListFilter) =>
    Promise.resolve(byStatus[f?.status ?? ""] ?? []),
  );
  retranscribeMock.mockReset();
  retranscribeMock.mockResolvedValue(undefined);
  clearFailedMock.mockReset();
  clearFailedMock.mockResolvedValue(0);
  getQueueCountsMock.mockReset();
  getQueueCountsMock.mockResolvedValue({ pending: 0, processing: 0, done: 0, failed: 0 });
  // restoreAllMocks (afterEach) blanks these module-level fns too — re-arm.
  listQueueMock.mockReset();
  listQueueMock.mockResolvedValue([]);
  queuePausedMock.mockReset();
  queuePausedMock.mockResolvedValue(false);
  handlers.length = 0;
});

afterEach(() => {
  document.querySelectorAll("ph-failed-panel, ph-queue-panel").forEach((el) => el.remove());
  vi.restoreAllMocks();
});

describe("FailedPanel rows", () => {
  it("renders one row per failed recording, newest first, with stage labels", async () => {
    byStatus.transcribe_failed = [rec("t1", { started_at: "2026-06-12T09:00:00Z" })];
    byStatus.hook_failed = [rec("h1", { status: "hook_failed", started_at: "2026-06-12T11:00:00Z" })];
    await mountPanel();
    const rows = qa(".failed-row");
    expect(rows).toHaveLength(2);
    // Newest (the hook failure) first, despite arriving from the second query.
    expect(rows[0].querySelector(".failed-stage")?.textContent).toBe("Hook");
    expect(rows[1].querySelector(".failed-stage")?.textContent).toBe("Transcription");
    expect(q(".failed-count-chip")?.textContent).toBe("2");
  });

  it("falls back to the timestamp when a recording has no title", async () => {
    const started = "2026-06-11T08:30:00Z";
    byStatus.transcribe_failed = [
      rec("titled", { title: "Standup notes" }),
      rec("untitled", { started_at: started }),
    ];
    await mountPanel();
    const titles = qa(".failed-title").map((t) => t.textContent);
    expect(titles).toContain("Standup notes");
    expect(titles).toContain(new Date(started).toLocaleString());
  });

  it("shows stored error_message, else the live-captured event error, else the fallback", async () => {
    recordFailureDetail("live-err", "transcribe", "boom from the wire");
    byStatus.transcribe_failed = [
      rec("stored-err", { error_message: "HTTP 400: audio too short" }),
      rec("live-err"),
      rec("mystery"),
    ];
    await mountPanel();
    const msgs = qa(".failed-msg").map((m) => m.textContent?.trim() ?? "");
    expect(msgs.some((m) => m.includes("HTTP 400: audio too short"))).toBe(true);
    expect(msgs.some((m) => m.includes("boom from the wire"))).toBe(true);
    expect(msgs.some((m) => m.includes("No error detail captured"))).toBe(true);
    // The fallback row is styled as such; the real messages aren't.
    expect(qa(".failed-msg.unknown")).toHaveLength(1);
  });

  it("shows the empty state when nothing has failed", async () => {
    await mountPanel();
    expect(qa(".failed-row")).toHaveLength(0);
    expect(q(".failed-empty")?.textContent).toContain("Nothing has failed");
    expect(q<HTMLButtonElement>(".failed-retry-all")?.disabled).toBe(true);
  });

  it("adds a row when a failure event arrives while the panel is open", async () => {
    await mountPanel();
    expect(qa(".failed-row")).toHaveLength(0);
    byStatus.transcribe_failed = [rec("fresh")];
    emit({ event: "transcription_failed", id: "fresh", error: "server said no" });
    await tick();
    const rows = qa(".failed-row");
    expect(rows).toHaveLength(1);
    expect(rows[0].querySelector(".failed-msg")?.textContent).toContain("server said no");
  });
});

describe("FailedPanel retry", () => {
  it("fires the retranscribe flow and removes the row optimistically", async () => {
    byStatus.transcribe_failed = [rec("a"), rec("b", { started_at: "2026-06-12T09:00:00Z" })];
    await mountPanel();
    expect(qa(".failed-row")).toHaveLength(2);
    qa(".failed-retry")[0].click();
    await tick();
    // The row left without any reload (listRecordings still returns both).
    expect(retranscribeMock).toHaveBeenCalledTimes(1);
    expect(retranscribeMock.mock.calls[0][0]).toBe("a");
    expect(qa(".failed-row")).toHaveLength(1);
    expect(qa(".failed-stage")).toHaveLength(1);
  });

  it("retries all sequentially, shows progress, and disables actions while running", async () => {
    byStatus.transcribe_failed = [rec("first", { started_at: "2026-06-12T12:00:00Z" })];
    byStatus.hook_failed = [rec("second", { status: "hook_failed", started_at: "2026-06-12T11:00:00Z" })];
    await mountPanel();

    const gates: Array<() => void> = [];
    retranscribeMock.mockImplementation(() => new Promise<void>((r) => { gates.push(r); }));

    q<HTMLButtonElement>(".failed-retry-all")!.click();
    await tick();

    // Strictly one at a time: the second call must wait for the first.
    expect(retranscribeMock).toHaveBeenCalledTimes(1);
    expect(retranscribeMock.mock.calls[0][0]).toBe("first");
    expect(q(".failed-foot-note")?.textContent).toContain("Retrying 0/2");
    expect(q<HTMLButtonElement>(".failed-retry-all")!.disabled).toBe(true);
    expect(q<HTMLButtonElement>(".failed-clear")!.disabled).toBe(true);
    expect(qa(".failed-retry").every((b) => (b as HTMLButtonElement).disabled)).toBe(true);

    gates[0]();
    await tick();
    expect(retranscribeMock).toHaveBeenCalledTimes(2);
    expect(retranscribeMock.mock.calls[1][0]).toBe("second");
    expect(q(".failed-foot-note")?.textContent).toContain("Retrying 1/2");

    byStatus.transcribe_failed = [];
    byStatus.hook_failed = [];
    gates[1]();
    await tick();
    await tick(); // the post-sweep reload settles
    expect(qa(".failed-row")).toHaveLength(0);
    expect(q(".failed-foot-note")?.textContent).not.toContain("Retrying");
  });
});

describe("FailedPanel footer", () => {
  it("keeps Clear failed working (quarantine only; rows stay)", async () => {
    byStatus.transcribe_failed = [rec("still-here")];
    getQueueCountsMock.mockResolvedValue({ pending: 0, processing: 0, done: 0, failed: 3 });
    clearFailedMock.mockResolvedValue(3);
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    await mountPanel();

    const clearBtn = q<HTMLButtonElement>(".failed-clear")!;
    expect(clearBtn.textContent).toContain("Clear failed (3)");
    clearBtn.click();
    await tick();

    expect(clearFailedMock).toHaveBeenCalledTimes(1);
    // The copy is honest: only the badge marker clears, failures stay visible.
    expect(confirmSpy.mock.calls[0][0]).toContain("keep");
    expect(confirmSpy.mock.calls[0][0]).toContain("Failed status");
    // The catalog rows are untouched — the list still shows the failure.
    expect(qa(".failed-row")).toHaveLength(1);
    expect(q<HTMLButtonElement>(".failed-clear")!.disabled).toBe(true);
  });

  it("does not clear when the confirmation is declined", async () => {
    getQueueCountsMock.mockResolvedValue({ pending: 0, processing: 0, done: 0, failed: 1 });
    vi.spyOn(window, "confirm").mockReturnValue(false);
    await mountPanel();
    q<HTMLButtonElement>(".failed-clear")!.click();
    await tick();
    expect(clearFailedMock).not.toHaveBeenCalled();
  });
});

describe("FailedPanel open + close", () => {
  it("Open selects the recording in the library and closes the panel", async () => {
    byStatus.transcribe_failed = [rec("jump-to")];
    let resolved = false;
    void openFailedPanel().then(() => { resolved = true; });
    await tick();
    const selected: string[] = [];
    const onSelect = (e: Event) => selected.push((e as CustomEvent<{ id: string }>).detail.id);
    window.addEventListener("phoneme:select-recording", onSelect);
    q<HTMLButtonElement>(".failed-open")!.click();
    await tick();
    window.removeEventListener("phoneme:select-recording", onSelect);
    expect(selected).toEqual(["jump-to"]);
    expect(panel()).toBeNull();
    expect(resolved).toBe(true);
  });

  it("Escape closes the panel", async () => {
    void openFailedPanel();
    await tick();
    expect(panel()).not.toBeNull();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await tick();
    expect(panel()).toBeNull();
  });
});

describe("Queue panel failed badge", () => {
  async function mountQueuePanel() {
    const el = document.createElement("ph-queue-panel");
    document.body.appendChild(el);
    await new Promise((r) => setTimeout(r, 0));
    await (el as HTMLElement & { updateComplete: Promise<unknown> }).updateComplete;
    return el;
  }

  it("hides the badge when nothing has failed", async () => {
    const el = await mountQueuePanel();
    expect(el.querySelector(".queue-failed")).toBeNull();
  });

  it("shows the count and opens the details panel on click", async () => {
    getQueueCountsMock.mockResolvedValue({ pending: 0, processing: 0, done: 0, failed: 2 });
    const el = await mountQueuePanel();
    const badge = el.querySelector<HTMLButtonElement>(".queue-failed");
    expect(badge).not.toBeNull();
    expect(badge!.textContent).toContain("2 failed");
    badge!.click();
    await tick();
    expect(panel()).not.toBeNull();
  });
});
