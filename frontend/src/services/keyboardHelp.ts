// The "?" cheat-sheet content: the keymap split into the three tiers documented
// in keyboard.ts (normal / vim nav / arrow nav). Pure data plus a tiny selector
// that orders the active nav-layer groups right after "Global"; keyboard.ts owns
// the overlay DOM and passes in the live nav flags.

export type HelpItem = { combo: string; label: string };
export type HelpGroup = { title: string; items: HelpItem[] };

const BASE_HELP_GROUPS: HelpGroup[] = [
  {
    title: "Global",
    items: [
      { combo: "/", label: "Focus search" },
      { combo: "?", label: "Show this help" },
      { combo: "g then l", label: "Go to Library" },
      { combo: "g then s", label: "Go to Settings" },
      { combo: "g then d", label: "Keyboard into the open recording" },
      { combo: "g then D", label: "Go to Doctor" },
      { combo: "g then A", label: "Toggle the AI-activity panel" },
      { combo: "g then /", label: "Highlight the search bar" },
      { combo: "g then b", label: "Go to / reveal the sidebar" },
      { combo: "g then 1 / 2", label: "Jump to the left / right split pane" },
      { combo: "g then T", label: "Open the Tag Manager" },
      { combo: "g then P", label: "Managers → Profiles" },
      { combo: "g then S", label: "Settings → Search" },
      { combo: "g then H", label: "Settings → Dictation history" },
      { combo: "Ctrl + ,", label: "Open Settings" },
      { combo: "Ctrl + B", label: "Toggle the sidebar" },
      { combo: "Ctrl + \\ / Ctrl + D", label: "Toggle the detail pane" },
      { combo: "Ctrl + /", label: "Hide / show the top bar" },
      { combo: "Ctrl + = / − / 0", label: "Zoom the list bigger / smaller / reset" },
      { combo: "Ctrl + Shift + = / −", label: "Bump the global UI text size" },
      { combo: "Ctrl + scroll", label: "Zoom the list (over the list pane)" },
      { combo: "Tab / Shift+Tab", label: "Move between controls / panes" },
      { combo: "Esc", label: "Close popups · leave search · leave Settings" },
    ],
  },
  {
    title: "Recordings list (when focused)",
    items: [
      { combo: "↑  ↓", label: "Move between recordings" },
      { combo: "Enter", label: "Open recording · fold/unfold a meeting" },
      { combo: "Shift + Enter", label: "Meeting title → open the merged view" },
      { combo: "Space", label: "Multi-select (on a meeting title: all tracks)" },
      { combo: "Shift + ↑ / ↓", label: "Extend the selection" },
      { combo: "Delete", label: "Delete the selection — all selected, else the open one (with Undo)" },
      { combo: "\\", label: "Split: cursor row (or two selected) beside the open one; on a meeting -> dual timeline" },
      { combo: "Esc", label: "Clear the multi-selection" },
    ],
  },
  {
    title: "Bulk actions bar (recordings selected)",
    items: [
      { combo: "Shift + Enter", label: "Hand the keyboard to the bar" },
      { combo: "h   l", label: "Move across the bar's buttons" },
      { combo: "Enter / Space", label: "Press the highlighted button" },
      { combo: "j · k · Esc", label: "Leave the bar" },
      { combo: "Ctrl+Shift+click ⠿", label: "Reset the bar's position" },
    ],
  },
  {
    title: "Open recording",
    items: [
      { combo: "p", label: "Play / pause" },
      { combo: "c", label: "Copy transcript" },
      { combo: "e", label: "Export transcript" },
      { combo: "r", label: "Re-run with chosen models (Models modal)" },
      { combo: "f", label: "Zen: full-window recording — or the list when nothing's open" },
      { combo: "t", label: "Add a tag (j/k browse · Enter adds)" },
      { combo: "Shift + t", label: "Open the Tag Manager" },
      { combo: "Ctrl + S", label: "Save the focused editor" },
      { combo: ":w  :wq  :q", label: "Save / save-and-leave / leave (vim editors)" },
      { combo: "Shift + Esc", label: "Leave the transcript / notes editor" },
    ],
  },
  {
    title: "Modals & popups (Re-run · Doctor · Tag Manager · confirmations)",
    items: [
      { combo: "Tab / Shift+Tab", label: "Move between the dialog's controls — always works, no nav layer needed" },
      { combo: "h l j k / arrows", label: "Rove the cursor over the controls (with vim or arrow nav on)" },
      { combo: "Enter / Space", label: "Activate the highlighted control (button fires · field opens to type)" },
      { combo: "Esc", label: "Close the dialog" },
    ],
  },
];

/** Shown in the help sheet only while `interface.vim_nav` is enabled. */
const VIM_HELP_GROUP: HelpGroup = {
  title: "Vim navigation (enabled)",
  items: [
    { combo: "h   l", label: "Move focus between sidebar / list / detail" },
    { combo: "j   k", label: "Move down / up (list · sidebar · detail rows)" },
    { combo: "k / ↑ at top", label: "Up into the search bar (↓ to come back)" },
    { combo: "h  l (header)", label: "Move across the header controls (wraps around)" },
    { combo: "Enter (header)", label: "Open the status / Record / Settings dropdown" },
    { combo: "j  k (in menu)", label: "Choose an option — Enter selects, Esc closes" },
    { combo: "g g", label: "Jump to the top (list · sidebar · detail)" },
    { combo: "G", label: "Jump to the bottom (list · sidebar · detail)" },
    { combo: "z z", label: "Center the list on the cursor row" },
    { combo: "x b   x /", label: "Toggle the sidebar / top bar (vim twins of Ctrl+B / Ctrl+/)" },
    { combo: "Enter", label: "Open recording · apply sidebar filter" },
    { combo: "j  k (sidebar)", label: "Filters · section headers · the queue's items" },
    { combo: "h  l (sidebar)", label: "Across a queue row's buttons (l past the end → list)" },
    { combo: "j  k (queue ▲▼)", label: "On a queue item's move pair: pick move-up / move-down" },
    { combo: "Enter (sidebar)", label: "Apply filter · fold a section · press a queue button" },
    { combo: "l (into detail)", label: "Enter the open recording, on the transcript" },
    { combo: "j  k (detail)", label: "Top row · actions · tags · transcript · views · notes" },
    { combo: "h  l (detail)", label: "Across a row's buttons (h at the start → list)" },
    { combo: "Enter (detail)", label: "Edit the box / press the button / open a dropdown" },
    { combo: "j k · Enter · Esc", label: "Drive a detail dropdown (Speed/Export/Views/Pipeline)" },
    { combo: "Enter (waveform)", label: "Scrub mode: h/l ±1s, H/L ±5s, Space play, Esc leaves" },
    { combo: "Enter (tag suggestion)", label: "Enter the chip: h/l pick ✓ apply / × dismiss, Enter acts, Esc backs out" },
    { combo: "h  l (split view)", label: "Cross between the two panes (at a row's edge)" },
    { combo: "Shift+Enter (tags)", label: "Open the Tag Manager" },
    { combo: "i", label: "Edit the transcript directly" },
    { combo: "d d", label: "Delete the selection — all selected, else the focused one (with Undo)" },
    { combo: "Esc", label: "Step back out a level" },
  ],
};

/** Shown in the help sheet only while `interface.arrow_nav` is enabled — the
 *  non-vim "normal" navigation layer driven entirely by the arrow keys. */
const ARROW_HELP_GROUP: HelpGroup = {
  title: "Arrow-key navigation (enabled)",
  items: [
    { combo: "← →", label: "Move focus between sidebar / list / detail panes" },
    { combo: "↑ ↓", label: "Move within the list · sidebar filters · detail rows" },
    { combo: "↑ at list top", label: "Rise into the header controls (↓ to come back)" },
    { combo: "← → (header)", label: "Move across the header controls" },
    { combo: "Enter", label: "Open / activate the focused row, button, or dropdown" },
    { combo: "Esc", label: "Step back out a level" },
  ],
};

export function helpGroups(arrowNav: boolean, vimNav: boolean): HelpGroup[] {
  // Surface the active nav layer(s) right after "Global" so they're the first
  // thing the user sees; hide them entirely when off, since the keys are inert.
  const layers: HelpGroup[] = [];
  if (arrowNav) layers.push(ARROW_HELP_GROUP);
  if (vimNav) layers.push(VIM_HELP_GROUP);
  return [BASE_HELP_GROUPS[0], ...layers, ...BASE_HELP_GROUPS.slice(1)];
}
