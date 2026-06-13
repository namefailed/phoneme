/**
 * The recording the detail pane is currently showing, as a tiny module
 * singleton. RecordingsView keeps it in sync; surfaces that aren't wired into
 * the view (e.g. the header's Quick Model Switcher) read it so their "Run once"
 * can target whatever recording the user is looking at. Holds a single
 * recording id only — meeting/session views clear it (a one-shot re-run needs a
 * concrete recording).
 */
let openId: string | null = null;

/** Record which recording the detail pane now shows (`null` = none / a
 *  meeting view). Called by RecordingsView on every select/deselect. */
export function setOpenRecordingId(id: string | null): void {
  openId = id;
}

/** The id of the recording open in the detail pane, or `null` when nothing
 *  (or a merged meeting view) is open. */
export function getOpenRecordingId(): string | null {
  return openId;
}
