import { describe, it, expect } from "vitest";
import { SectionDiarization, diarizationMismatch } from "./SectionDiarization";

/** Mount a SectionDiarization into a fresh host with the given config. */
function mount(config: Record<string, unknown> = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  new SectionDiarization(host, config);
  return host;
}

/** The diarization provider <select> and its option values. */
function providerSelect(host: HTMLElement) {
  return host.querySelector<HTMLSelectElement>(
    `[data-key="diarization.provider"]`,
  )!;
}
function optionValues(host: HTMLElement): string[] {
  return Array.from(providerSelect(host).options).map((o) => o.value);
}

describe("diarizationMismatch — warns when the combo can't run", () => {
  it("returns no warning when diarization is off or unset", () => {
    expect(diarizationMismatch("none", "deepgram")).toBeNull();
    expect(diarizationMismatch("", "openai")).toBeNull();
  });

  it("warns for cloud diarization when the STT provider differs", () => {
    expect(diarizationMismatch("deepgram", "local")).toMatch(/Deepgram/);
    expect(diarizationMismatch("deepgram", "openai")).toMatch(/Deepgram/);
    expect(diarizationMismatch("assemblyai", "local")).toMatch(/AssemblyAI/);
    expect(diarizationMismatch("assemblyai", "groq")).toMatch(/AssemblyAI/);
  });

  it("does NOT warn when the cloud diarization provider matches STT", () => {
    expect(diarizationMismatch("deepgram", "deepgram")).toBeNull();
    expect(diarizationMismatch("assemblyai", "assemblyai")).toBeNull();
  });

  it("allows local diarization on any OpenAI-compatible STT", () => {
    expect(diarizationMismatch("local", "local")).toBeNull();
    expect(diarizationMismatch("local", "openai")).toBeNull();
    expect(diarizationMismatch("local", "groq")).toBeNull();
    expect(diarizationMismatch("local", "custom")).toBeNull();
  });

  it("warns for local diarization on a provider that returns no segments", () => {
    expect(diarizationMismatch("local", "deepgram")).toMatch(/Local diarization/);
    expect(diarizationMismatch("local", "assemblyai")).toMatch(/Local diarization/);
  });
});

describe("SectionDiarization — provider dropdown", () => {
  it("renders all four provider options", () => {
    const host = mount({ diarization: { provider: "none" } });
    expect(optionValues(host)).toEqual([
      "none",
      "local",
      "deepgram",
      "assemblyai",
    ]);
  });

  it("pre-selects the configured provider", () => {
    const host = mount({ diarization: { provider: "deepgram" } });
    expect(providerSelect(host).value).toBe("deepgram");
  });

  it("defaults to 'none' when no diarization config is present", () => {
    const host = mount({});
    expect(providerSelect(host).value).toBe("none");
  });

  it("round-trips a cloud pick back into config.diarization.provider", () => {
    const config: Record<string, unknown> = { diarization: { provider: "none" } };
    const host = mount(config);
    const select = providerSelect(host);

    select.value = "assemblyai";
    select.dispatchEvent(new Event("change"));
    expect((config.diarization as { provider: string }).provider).toBe(
      "assemblyai",
    );

    select.value = "deepgram";
    select.dispatchEvent(new Event("change"));
    expect((config.diarization as { provider: string }).provider).toBe(
      "deepgram",
    );
  });
});

describe("SectionDiarization — mismatch warning box toggles", () => {
  const warnVisible = (host: HTMLElement) =>
    host.querySelector<HTMLElement>("#diarize-warn")!.style.display !== "none";
  const warnText = (host: HTMLElement) =>
    host.querySelector<HTMLElement>("#diarize-warn-text")!.textContent ?? "";

  it("is hidden when diarization is off", () => {
    const host = mount({
      whisper: { provider: "local" },
      diarization: { provider: "none" },
    });
    expect(warnVisible(host)).toBe(false);
  });

  it("is hidden when cloud diarization matches the STT provider", () => {
    const host = mount({
      whisper: { provider: "deepgram" },
      diarization: { provider: "deepgram" },
    });
    expect(warnVisible(host)).toBe(false);
  });

  it("shows a warning when cloud diarization differs from the STT provider", () => {
    const host = mount({
      whisper: { provider: "local" },
      diarization: { provider: "deepgram" },
    });
    expect(warnVisible(host)).toBe(true);
    expect(warnText(host)).toMatch(/Deepgram/);
  });

  it("toggles the warning live as the provider selection changes", () => {
    const config = {
      whisper: { provider: "local" },
      diarization: { provider: "none" },
    };
    const host = mount(config);
    const select = providerSelect(host);

    // none → no warning
    expect(warnVisible(host)).toBe(false);

    // deepgram on a local STT → warning fires
    select.value = "deepgram";
    select.dispatchEvent(new Event("change"));
    expect(warnVisible(host)).toBe(true);
    expect(warnText(host)).toMatch(/Deepgram/);

    // local diarization on a local STT → warning clears
    select.value = "local";
    select.dispatchEvent(new Event("change"));
    expect(warnVisible(host)).toBe(false);
  });
});
