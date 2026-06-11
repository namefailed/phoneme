/**
 * The recording the detail pane is currently showing, as a tiny module
 * singleton. RecordingsView keeps it in sync; surfaces that aren't wired into
 * the view (e.g. the header's Quick Model Switcher) read it so their "Run once"
 * can target whatever recording the user is looking at. Holds a single
 * recording id only — meeting/session views clear it (a one-shot re-run needs a
 * concrete recording).
 */
let openId: string | null = null;

export function setOpenRecordingId(id: string | null): void {
  openId = id;
}

export function getOpenRecordingId(): string | null {
  return openId;
}
