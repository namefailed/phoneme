import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("../../services/ipc", () => ({
  getWords: vi.fn(),
}));

import { getWords, type TranscriptWord } from "../../services/ipc";
import { SyncedTranscript, activeWordIndex } from "./SyncedTranscript";

const WORDS: TranscriptWord[] = [
  { idx: 0, start_ms: 0, end_ms: 500, text: "hello", speaker: "1", confidence: 0.97 },
  { idx: 1, start_ms: 500, end_ms: 1500, text: "there", speaker: "1", confidence: null },
  // A gap [1500, 2000) before the next speaker's first word at 2000.
  { idx: 2, start_ms: 2000, end_ms: 2600, text: "hi,", speaker: "2", confidence: 0.9 },
  { idx: 3, start_ms: 2600, end_ms: 4000, text: "thanks", speaker: "2", confidence: 0.8 },
];

/** Flush the constructor's async load. */
const tick = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  vi.mocked(getWords).mockReset();
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("activeWordIndex", () => {
  it("finds the word whose [start,end) window holds a time", () => {
    expect(activeWordIndex(WORDS, -10)).toBe(-1);
    expect(activeWordIndex(WORDS, 0)).toBe(0);
    expect(activeWordIndex(WORDS, 499)).toBe(0);
    expect(activeWordIndex(WORDS, 500)).toBe(1);
    expect(activeWordIndex(WORDS, 2000)).toBe(2);
    expect(activeWordIndex(WORDS, 3999)).toBe(3);
    expect(activeWordIndex([], 100)).toBe(-1);
  });

  it("holds the last started word through a gap, then jumps at the next start", () => {
    // 1700 sits in the [1500,2000) gap → still the word that last started (idx 1).
    expect(activeWordIndex(WORDS, 1700)).toBe(1);
    // After the very last word's end, stays on the last word.
    expect(activeWordIndex(WORDS, 9999)).toBe(3);
  });
});

describe("SyncedTranscript", () => {
  it("renders one clickable span per word, grouped into speaker paragraphs", async () => {
    vi.mocked(getWords).mockResolvedValue(WORDS);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new SyncedTranscript(host, "rec-1", { onSeek: vi.fn() });
    await tick();

    const spans = host.querySelectorAll(".st-word");
    expect(spans.length).toBe(4);
    expect(spans[0].textContent).toBe("hello");
    expect(spans[3].textContent).toBe("thanks");
    // Two speakers → two paragraphs, each with its speaker label.
    const paras = host.querySelectorAll(".st-para");
    expect(paras.length).toBe(2);
    const speakers = [...host.querySelectorAll(".st-speaker")].map((el) => el.textContent);
    expect(speakers).toEqual(["Speaker 1", "Speaker 2"]);
    view.dispose();
  });

  it("joins subword/punctuation tokens without a leading space", async () => {
    // whisper emits subword + punctuation tokens; only word-starts carry
    // leading_space. The flow must read "I overstepped?" not "I over ste pped ?".
    vi.mocked(getWords).mockResolvedValue([
      { idx: 0, start_ms: 0, end_ms: 300, text: "I", speaker: "1", leading_space: true },
      { idx: 1, start_ms: 300, end_ms: 600, text: "over", speaker: "1", leading_space: true },
      { idx: 2, start_ms: 600, end_ms: 700, text: "ste", speaker: "1", leading_space: false },
      { idx: 3, start_ms: 700, end_ms: 800, text: "pped", speaker: "1", leading_space: false },
      { idx: 4, start_ms: 800, end_ms: 850, text: "?", speaker: "1", leading_space: false },
    ]);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new SyncedTranscript(host, "rec-1", { onSeek: vi.fn() });
    await tick();
    expect(host.querySelector(".st-words")?.textContent).toBe("I overstepped?");
    view.dispose();
  });

  it("marks only words scored below LOW_CONFIDENCE, leaving unscored ones plain", async () => {
    // Provider confidence: 0.97 (high), null (unscored), 0.3 (low), 0.8 (high).
    // Only the 0.3 word should get the squiggle; the null one stays unflagged.
    vi.mocked(getWords).mockResolvedValue([
      { idx: 0, start_ms: 0, end_ms: 500, text: "clear", speaker: "1", confidence: 0.97 },
      { idx: 1, start_ms: 500, end_ms: 1000, text: "unscored", speaker: "1", confidence: null },
      { idx: 2, start_ms: 1000, end_ms: 1500, text: "mumbled", speaker: "1", confidence: 0.3 },
      { idx: 3, start_ms: 1500, end_ms: 2000, text: "fine", speaker: "1", confidence: 0.8 },
    ]);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new SyncedTranscript(host, "rec-1", { onSeek: vi.fn() });
    await tick();

    const low = host.querySelectorAll(".st-word.st-low-conf");
    expect(low.length).toBe(1);
    expect((low[0] as HTMLElement).textContent).toBe("mumbled");
    // The percentage hint rides along in the tooltip.
    expect((low[0] as HTMLElement).title).toContain("30% confidence");
    // The unscored word stays plain — no misleading "low confidence" mark.
    const unscored = host.querySelector('.st-word[data-idx="1"]') as HTMLElement;
    expect(unscored.classList.contains("st-low-conf")).toBe(false);
    view.dispose();
  });

  it("maps numeric speaker labels through the recording's custom names", async () => {
    vi.mocked(getWords).mockResolvedValue(WORDS);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new SyncedTranscript(host, "rec-1", {
      speakerNames: [{ speaker_label: 2, name: "Sarah" }],
      onSeek: vi.fn(),
    });
    await tick();
    const speakers = [...host.querySelectorAll(".st-speaker")].map((el) => el.textContent);
    expect(speakers).toEqual(["Speaker 1", "Sarah"]);
    view.dispose();
  });

  it("clicking a word seeks to its start (in seconds)", async () => {
    vi.mocked(getWords).mockResolvedValue(WORDS);
    const onSeek = vi.fn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new SyncedTranscript(host, "rec-1", { onSeek });
    await tick();

    (host.querySelectorAll(".st-word")[2] as HTMLElement).click();
    expect(onSeek).toHaveBeenCalledWith(2); // 2000ms → 2.0s
    view.dispose();
  });

  it("highlights the word under the playhead on a time update, and only it", async () => {
    vi.mocked(getWords).mockResolvedValue(WORDS);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new SyncedTranscript(host, "rec-1", { onSeek: vi.fn() });
    await tick();

    view.setPlaybackTime(0.6); // 600ms → inside word idx 1 [500,1500)
    let active = host.querySelectorAll(".st-word.st-active");
    expect(active.length).toBe(1);
    expect((active[0] as HTMLElement).dataset.idx).toBe("1");

    // Advancing the playhead moves the highlight to the next word and clears the old.
    view.setPlaybackTime(2.1); // 2100ms → inside word idx 2 [2000,2600)
    active = host.querySelectorAll(".st-word.st-active");
    expect(active.length).toBe(1);
    expect((active[0] as HTMLElement).dataset.idx).toBe("2");
    view.dispose();
  });

  it("shows the retranscribe hint when no word timings are stored", async () => {
    vi.mocked(getWords).mockResolvedValue([]);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new SyncedTranscript(host, "rec-1", { onSeek: vi.fn() });
    await tick();
    expect(host.querySelector(".st-empty")?.textContent).toContain("Transcribe");
    expect(host.querySelectorAll(".st-word").length).toBe(0);
    view.dispose();
  });

  it("never edits — dispose empties the host and leaves no live spans", async () => {
    vi.mocked(getWords).mockResolvedValue(WORDS);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new SyncedTranscript(host, "rec-1", { onSeek: vi.fn() });
    await tick();
    expect(host.querySelectorAll(".st-word").length).toBe(4);
    view.dispose();
    expect(host.innerHTML).toBe("");
  });
});
