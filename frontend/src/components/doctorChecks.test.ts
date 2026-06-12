import { describe, it, expect } from "vitest";
import {
  categoryMeta,
  checkGroup,
  fixAllPlan,
  groupChecks,
  healthCounts,
  type DoctorCheckInfo,
} from "./doctorChecks";

const check = (over: Partial<DoctorCheckInfo>): DoctorCheckInfo => ({
  name: "x",
  ok: false,
  detail: "d",
  ...over,
});

describe("categoryMeta", () => {
  it("maps explicit categories to badge label + class", () => {
    expect(categoryMeta(check({ category: "critical" }))).toEqual({
      label: "Critical",
      cls: "critical",
    });
    expect(categoryMeta(check({ category: "warning" }))).toEqual({
      label: "Warning",
      cls: "warning",
    });
    expect(categoryMeta(check({ category: "info" }))).toEqual({ label: "Info", cls: "info" });
  });

  it("defaults a failing check without a category to warning (older daemons)", () => {
    expect(categoryMeta(check({ ok: false }))).toEqual({ label: "Warning", cls: "warning" });
  });

  it("defaults a passing check without a category to info", () => {
    expect(categoryMeta(check({ ok: true }))).toEqual({ label: "Info", cls: "info" });
  });
});

describe("fixAllPlan", () => {
  it("collects failing checks' fix actions top-down", () => {
    const plan = fixAllPlan([
      check({ ok: false, fix_action: "start_daemon" }),
      check({ ok: false, fix_action: "restart_whisper" }),
      check({ ok: false, fix_action: "open_hooks_folder" }),
    ]);
    expect(plan).toEqual(["start_daemon", "restart_whisper", "open_hooks_folder"]);
  });

  it("skips passing checks and checks without an action", () => {
    const plan = fixAllPlan([
      check({ ok: true, fix_action: "open_config" }),
      check({ ok: false, fix_action: null }),
      check({ ok: false }),
      check({ ok: false, fix_action: "restart_whisper" }),
    ]);
    expect(plan).toEqual(["restart_whisper"]);
  });

  it("runs a shared action once (whisper + preview both restart_whisper)", () => {
    const plan = fixAllPlan([
      check({ name: "Whisper server", ok: false, fix_action: "restart_whisper" }),
      check({ name: "Live-preview server", ok: false, fix_action: "restart_whisper" }),
      check({ name: "Hook command", ok: false, fix_action: "open_hooks_folder" }),
    ]);
    expect(plan).toEqual(["restart_whisper", "open_hooks_folder"]);
  });

  it("is empty when everything passes", () => {
    expect(fixAllPlan([check({ ok: true, fix_action: "open_config" })])).toEqual([]);
    expect(fixAllPlan([])).toEqual([]);
  });
});

describe("checkGroup", () => {
  it("maps every known check name to its subsystem", () => {
    expect(checkGroup("Daemon")).toBe("Servers");
    expect(checkGroup("Whisper server")).toBe("Servers");
    expect(checkGroup("Live-preview server")).toBe("Servers");
    expect(checkGroup("Ollama (optional)")).toBe("Servers");
    expect(checkGroup("Whisper model file")).toBe("Models");
    expect(checkGroup("Live-preview model")).toBe("Models");
    expect(checkGroup("Semantic search model")).toBe("Models");
    expect(checkGroup("Diarization models")).toBe("Models");
    expect(checkGroup("Audio directory")).toBe("Storage");
    expect(checkGroup("Disk space (recordings)")).toBe("Storage");
    expect(checkGroup("Disk space (app data)")).toBe("Storage");
    expect(checkGroup("Config file")).toBe("Configuration");
    expect(checkGroup("Hook command")).toBe("Configuration");
  });

  it("groups the provider-aware connection checks, including dynamic LLM names", () => {
    expect(checkGroup("Transcription API key")).toBe("Servers");
    expect(checkGroup("Dictation STT endpoint")).toBe("Servers");
    expect(checkGroup("LLM endpoint (cleanup, summary, titles)")).toBe("Servers");
    expect(checkGroup("LLM API key (tags)")).toBe("Servers");
  });

  it("falls back to Other for names it doesn't know", () => {
    expect(checkGroup("Brand-new check")).toBe("Other");
    expect(checkGroup("")).toBe("Other");
    // The table maps the current backend names only; the pre-rename
    // "Diarization model" (singular) from older daemons still renders,
    // just ungrouped.
    expect(checkGroup("Diarization model")).toBe("Other");
  });
});

describe("groupChecks", () => {
  it("buckets in display order, keeping check order within each group", () => {
    const grouped = groupChecks([
      check({ name: "Config file", ok: true }),
      check({ name: "Whisper server", ok: true }),
      check({ name: "Audio directory", ok: true }),
      check({ name: "Mystery probe", ok: true }),
      check({ name: "Ollama (optional)", ok: true }),
    ]);
    expect(grouped.map((g) => g.group)).toEqual(["Servers", "Storage", "Configuration", "Other"]);
    expect(grouped[0].checks.map((c) => c.name)).toEqual(["Whisper server", "Ollama (optional)"]);
    expect(grouped[3].checks.map((c) => c.name)).toEqual(["Mystery probe"]);
  });

  it("drops empty groups", () => {
    expect(groupChecks([])).toEqual([]);
    expect(groupChecks([check({ name: "Diarization models" })]).map((g) => g.group)).toEqual([
      "Models",
    ]);
  });
});

describe("healthCounts", () => {
  it("tallies passing checks and failures per category", () => {
    expect(
      healthCounts([
        check({ ok: true }),
        check({ ok: true }),
        check({ ok: false, category: "critical" }),
        check({ ok: false, category: "warning" }),
        check({ ok: false, category: "info" }),
      ]),
    ).toEqual({ total: 5, passing: 2, critical: 1, warning: 1, info: 1 });
  });

  it("counts an uncategorized failure as warning (older daemons)", () => {
    expect(healthCounts([check({ ok: false })]).warning).toBe(1);
  });

  it("is all zeroes on no checks", () => {
    expect(healthCounts([])).toEqual({ total: 0, passing: 0, critical: 0, warning: 0, info: 0 });
  });
});
