/**
 * Shared app-health store. Runs the Doctor checks once on a single 30s timer and
 * fans the result out to every subscriber — the header pill, the Settings pill,
 * and the header's failure banner — so they all read one source of truth and the
 * checks (which include backend probes) never run more than once per interval.
 *
 * Polling is owned here, not by any view, so the pill stays live even in the
 * Settings page (where the header bar is hidden). Checks are skipped while the
 * window is hidden — a minimized/tray window shouldn't keep probing — and a check
 * that came due while hidden runs the moment the window shows again.
 */
import { runDoctor } from "../services/ipc";

export type HealthLevel = "ok" | "bad" | "unknown";
export interface HealthIssue {
  name: string;
  fix: string | null;
}
export interface HealthSnapshot {
  level: HealthLevel;
  issues: HealthIssue[];
}

let snapshot: HealthSnapshot = { level: "unknown", issues: [] };
const subscribers = new Set<(s: HealthSnapshot) => void>();
let timer: number | null = null;
let dueWhileHidden = false;
let started = false;

export function getHealth(): HealthSnapshot {
  return snapshot;
}

/** Subscribe to health changes; the callback fires immediately with the current
 *  snapshot. Returns an unsubscribe function. */
export function subscribeHealth(cb: (s: HealthSnapshot) => void): () => void {
  subscribers.add(cb);
  cb(snapshot);
  return () => {
    subscribers.delete(cb);
  };
}

function emit() {
  for (const cb of [...subscribers]) cb(snapshot);
}

/** Run the shared Doctor checks and distill app health. A check marked
 *  "(optional)" never fails health; a daemon that can't even answer is the
 *  reddest possible state. */
export async function refreshHealth(): Promise<void> {
  try {
    const checks = await runDoctor();
    const failing = checks.filter((c) => !c.ok && !c.name.toLowerCase().includes("(optional)"));
    snapshot = {
      level: failing.length ? "bad" : "ok",
      issues: failing.map((c) => ({ name: c.name, fix: c.fix_action ?? null })),
    };
  } catch {
    snapshot = { level: "bad", issues: [{ name: "Daemon not reachable", fix: "start_daemon" }] };
  }
  emit();
}

/** Start the shared poll (idempotent — safe to call from every consumer). */
export function startHealthPolling(): void {
  if (started) return;
  started = true;
  void refreshHealth();
  timer = window.setInterval(() => {
    if (document.visibilityState === "hidden") {
      dueWhileHidden = true;
      return;
    }
    void refreshHealth();
  }, 30000);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible" && dueWhileHidden) {
      dueWhileHidden = false;
      void refreshHealth();
    }
  });
}
