/**
 * Last-known app health, shared so the Settings panel's floating Doctor pill can
 * paint the right colour the INSTANT it mounts — instead of flashing grey
 * ("unknown") then turning green a second later when its own check resolves.
 *
 * The header's `ph-header-bar` owns the live 30s polling pill and writes here on
 * every check ([`setHealth`]); the Settings view reads [`getHealth`] on open for
 * the initial dot, then refreshes with its own one-shot check (usually a no-op).
 */
export type Health = "ok" | "bad" | "unknown";

let health: Health = "unknown";
let issueCount = 0;

/** Record the latest health (called by the header after each Doctor check). */
export function setHealth(next: Health, count: number): void {
  health = next;
  issueCount = count;
}

/** The last-known health, for an instant initial paint with no grey flash. */
export function getHealth(): { health: Health; issueCount: number } {
  return { health, issueCount };
}
