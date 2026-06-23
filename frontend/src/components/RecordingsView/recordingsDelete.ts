//! The grace-period "undoable delete" flows, lifted out of RecordingsView's
//! orchestrator (index.ts). Rows hide immediately; the real (permanent) delete
//! only fires when the Undo toast lapses, so a stray keystroke never loses data.
//! Kept as free functions over a small structural {@link DeleteHost} so they
//! don't depend on the host's concrete type — the host wires its list/store/
//! refresh in.

import { showActionToast, showToast } from "../../utils/toast";

/** The slice of RecordingsView the delete flows touch. */
export interface DeleteHost {
  list: { setPendingDelete(ids: string[], hidden: boolean): void; clearSelection(): void };
  /** The currently-open recording id (or null). */
  selectedId(): string | null;
  /** The loaded recordings (for resolving a meeting session's member tracks). */
  recordings(): { id: string; meeting_id?: string | null }[];
  /** Close the detail pane (when the open recording is being deleted). */
  deselect(): void;
  /** Re-query the list after the real delete lands. */
  refresh(): Promise<void>;
}

/** Hide `ids` now; delete them (with the chosen `keepAudio`) only when the Undo
 *  toast lapses. Undo just un-hides — nothing reached the backend. */
export function runUndoableDelete(host: DeleteHost, ids: string[], keepAudio: boolean): void {
  // Optimistically hide the rows, drop them from the selection (so the bulk bar
  // count stays honest), and close the detail if the open one is going.
  host.list.setPendingDelete(ids, true);
  host.list.clearSelection();
  const sel = host.selectedId();
  if (sel && ids.includes(sel)) host.deselect();

  const noun = ids.length === 1 ? "Recording" : `${ids.length} recordings`;
  const label = keepAudio ? `${noun} removed — audio kept` : `${noun} deleted`;
  showActionToast({
    message: label,
    actionLabel: "Undo",
    icon: "🗑",
    durationMs: 6000,
    onAction: () => {
      // Cancelled — just un-hide; nothing was ever sent to the backend.
      host.list.setPendingDelete(ids, false);
    },
    onExpire: async () => {
      const { deleteRecording } = await import("../../services/ipc");
      const failed: string[] = [];
      for (const id of ids) {
        try {
          await deleteRecording(id, keepAudio);
        } catch (err) {
          console.error("Failed to delete recording:", err);
          failed.push(id);
        }
      }
      // Reconcile the store first — the re-fetch drops the now-deleted rows (the
      // daemon removes the catalog row before `deleteRecording` resolves), then
      // clear the hide set. Clearing it before the refresh lands would briefly
      // un-hide rows still in the store, flashing them back right before they go.
      await host.refresh();
      host.list.setPendingDelete(ids, false);
      // A failed delete un-hides the row (it's still in the store), but the
      // grace-period toast already showed "deleted" and dismissed itself — so say
      // it plainly instead of leaving that misleading success as the only feedback.
      if (failed.length) {
        showToast(
          failed.length === 1
            ? "Couldn't delete the recording — it's still here."
            : `Couldn't delete ${failed.length} recordings — they're still here.`,
          "error",
        );
      }
    },
  });
}

/** Grace-period delete of a whole meeting session: hide its member tracks now,
 *  then fire a single `DeleteSession` (every track at once) when the Undo toast
 *  lapses. The session header isn't a deletable list row, so the hide is keyed by
 *  the member track ids from the store. */
export function runUndoableSessionDelete(
  host: DeleteHost,
  sessionId: string,
  keepAudio: boolean,
): void {
  const meetingId = sessionId.replace("session:", "");
  const trackIds = host
    .recordings()
    .filter((r) => r.meeting_id === meetingId)
    .map((r) => r.id);
  host.list.setPendingDelete(trackIds, true);
  host.list.clearSelection();
  const sel = host.selectedId();
  if (sel === sessionId || (sel && trackIds.includes(sel))) host.deselect();

  const label = keepAudio ? "Meeting removed — audio kept" : "Meeting deleted";
  showActionToast({
    message: label,
    actionLabel: "Undo",
    icon: "🗑",
    durationMs: 6000,
    onAction: () => {
      host.list.setPendingDelete(trackIds, false);
    },
    onExpire: async () => {
      const { deleteSession } = await import("../../services/ipc");
      let failed = false;
      try {
        await deleteSession(meetingId, keepAudio);
      } catch (err) {
        console.error("Failed to delete meeting session:", err);
        failed = true;
      }
      // Reconcile first (the daemon already dropped the rows), then un-hide —
      // same ordering as the per-recording flow to avoid a flash-back.
      await host.refresh();
      host.list.setPendingDelete(trackIds, false);
      if (failed) {
        showToast("Couldn't delete the meeting — it's still here.", "error");
      }
    },
  });
}
