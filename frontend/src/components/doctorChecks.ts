/**
 * Shared bits for the two Doctor surfaces (DoctorModal + DoctorView): the
 * check shape with the category fields the backend emits, badge metadata,
 * and the "Fix All" plan. Kept local to the Doctor components on purpose —
 * the shared ipc.ts DoctorCheck type stays minimal, and these fields are
 * additive (older daemons simply don't send them).
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
