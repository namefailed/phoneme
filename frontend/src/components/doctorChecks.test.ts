import { describe, it, expect } from "vitest";
import { categoryMeta, fixAllPlan, type DoctorCheckInfo } from "./doctorChecks";

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
