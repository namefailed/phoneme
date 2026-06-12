/**
 * Shared bits for the two Doctor surfaces (DoctorModal + DoctorView): the
 * check shape with the category fields the backend emits, badge metadata,
 * the "Fix All" plan, the health-strip tallies and the subsystem grouping
 * for the collapsed passing section. Kept local to the Doctor components on
 * purpose — the shared ipc.ts DoctorCheck type stays minimal, and these
 * fields are additive (older daemons simply don't send them).
 */

export type DoctorCategory = "critical" | "warning" | "info";

/** One Doctor check with the category/explanation/fix-hint extensions. */
export type DoctorCheckInfo = {
  name: string;
  ok: boolean;
  detail: string;
  /** Opaque token the GUI maps to a "Fix" action (e.g. open_config). */
  fix_action?: string | null;
  /** Severity of the current state; absent on results from older daemons. */
  category?: DoctorCategory;
  /** One sentence: what the check verifies and why it matters. */
  explanation?: string;
  /** Actionable next step, present when the check fails with a known remedy. */
  fix_hint?: string | null;
};

/**
 * Badge label + CSS modifier for a check's category. Failing checks from
 * daemons that predate categories default to "warning" so they stay visible;
 * passing checks default to "info".
 */
export function categoryMeta(check: Pick<DoctorCheckInfo, "ok" | "category">): {
  label: string;
  cls: DoctorCategory;
} {
  const cat: DoctorCategory = check.category ?? (check.ok ? "info" : "warning");
  const label = cat === "critical" ? "Critical" : cat === "warning" ? "Warning" : "Info";
  return { label, cls: cat };
}

/**
 * The ordered fix plan for "Fix All": every failing check's fix_action,
 * top-down, each distinct action once (the whisper + preview probes share
 * "restart_whisper" — one restart sweeps and respawns both servers).
 */
export function fixAllPlan(checks: DoctorCheckInfo[]): string[] {
  const seen = new Set<string>();
  const plan: string[] = [];
  for (const c of checks) {
    if (c.ok || !c.fix_action || seen.has(c.fix_action)) continue;
    seen.add(c.fix_action);
    plan.push(c.fix_action);
  }
  return plan;
}

/** Display order of the subsystem groups in the collapsed passing section. */
export const CHECK_GROUPS = ["Servers", "Models", "Storage", "Configuration", "Other"] as const;
export type CheckGroup = (typeof CHECK_GROUPS)[number];

/**
 * Subsystem for each check, by the exact name the backend emits
 * (crates/phoneme-core/src/doctor.rs, plus the GUI-side "Daemon" check).
 * Names not listed here land in "Other", so a new backend check shows up
 * ungrouped instead of vanishing.
 */
const GROUP_BY_NAME: Record<string, CheckGroup> = {
  Daemon: "Servers",
  "Whisper server": "Servers",
  "Live-preview server": "Servers",
  "Ollama (optional)": "Servers",
  "Whisper model file": "Models",
  "Live-preview model": "Models",
  "Semantic search model": "Models",
  "Diarization models": "Models",
  "Audio directory": "Storage",
  "Disk space (recordings)": "Storage",
  "Disk space (app data)": "Storage",
  "Config file": "Configuration",
  "Hook command": "Configuration",
};

/** The subsystem group a check name belongs to; unknown names go to "Other". */
export function checkGroup(name: string): CheckGroup {
  return GROUP_BY_NAME[name] ?? "Other";
}

/**
 * Bucket checks by subsystem in `CHECK_GROUPS` order, preserving the checks'
 * own order inside each bucket. Empty groups are dropped.
 */
export function groupChecks(
  checks: DoctorCheckInfo[],
): { group: CheckGroup; checks: DoctorCheckInfo[] }[] {
  const buckets = new Map<CheckGroup, DoctorCheckInfo[]>();
  for (const c of checks) {
    const g = checkGroup(c.name);
    const list = buckets.get(g);
    if (list) list.push(c);
    else buckets.set(g, [c]);
  }
  return CHECK_GROUPS.filter((g) => buckets.has(g)).map((g) => ({
    group: g,
    checks: buckets.get(g)!,
  }));
}

/** What the health strip shows: failing counts per category, plus pass/total. */
export type HealthCounts = {
  total: number;
  passing: number;
  /** Failing checks per category (uncategorized failures count as warning). */
  critical: number;
  warning: number;
  info: number;
};

/** Tally checks for the health strip's chips. */
export function healthCounts(checks: DoctorCheckInfo[]): HealthCounts {
  const counts: HealthCounts = {
    total: checks.length,
    passing: 0,
    critical: 0,
    warning: 0,
    info: 0,
  };
  for (const c of checks) {
    if (c.ok) counts.passing += 1;
    else counts[categoryMeta(c).cls] += 1;
  }
  return counts;
}
