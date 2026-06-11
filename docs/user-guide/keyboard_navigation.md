# ⌨️ Keyboard Navigation

Phoneme is fully drivable from the keyboard. Press **`?`** anywhere for the
built-in cheat sheet (it shows exactly what's active for your settings). The
vim-style pane navigation is opt-in: **Settings → Appearance → Vim navigation**
(`interface.vim_nav`); everything else below works for everyone.

## Global

| Keys | Action |
|------|--------|
| `/` | Focus the search box |
| `?` | Open the shortcut cheat sheet |
| `g` then `l` | Go to the Library |
| `g` then `s` | Go to Settings |
| `g` then `d` | Go to the Doctor |
| `g` then `T` | Open the Tag Manager (quick popup) |
| `g` then `P` | Settings → Managers → Profiles |
| `g` then `S` | Settings → Managers → Saved searches |
| `Ctrl + ,` | Open Settings |
| `Ctrl + B` | Toggle the sidebar |
| `Ctrl + \` | Toggle the detail pane |
| `Ctrl + =` / `Ctrl + -` / `Ctrl + 0` | Zoom the recordings list bigger / smaller / reset |
| `Ctrl + scroll` | Zoom the list (while the pointer is over it) |
| `Esc` | Close popups · step back out a level |

The list zoom is clamped to 60–200% and remembered per device.

## Recordings list

| Keys | Action |
|------|--------|
| `↑` `↓` (or `j` `k` with vim nav) | Move between rows — including meeting title rows |
| `Enter` | Open the recording · fold/unfold a meeting |
| `Shift + Enter` | On a meeting title: open the **merged conversation view** |
| `Space` | Toggle multi-select (on a meeting title: all of its tracks) |
| `Shift + ↑/↓` | Extend the multi-selection |
| `\` | With exactly **two** recordings selected: open them **side by side** |
| `d` `d` (vim nav) | Delete the focused recording (with Undo) |
| `g g` / `G` (vim nav) | Jump to the first / last row |
| `Esc` | Clear the multi-selection |

## Bulk actions bar (shown while recordings are selected)

| Keys | Action |
|------|--------|
| `Shift + Enter` | Hand the keyboard to the bar |
| `h` `l` | Move across the bar's buttons |
| `Enter` / `Space` | Press the highlighted button |
| `j` · `k` · `Esc` | Leave the bar |
| `Ctrl + Shift + click` the ⠿ grip | Reset the bar to its default position |

## Open recording

| Keys | Action |
|------|--------|
| `p` | Play / pause |
| `c` | Copy the transcript |
| `e` | Export the transcript |
| `r` | Re-run with chosen models (opens the Models modal on "Run once") |
| `f` | Full-screen focus mode |
| `t` | Focus the add-tag box (`j`/`k` browse suggestions · `Enter` adds) |
| `Shift + T` | Open the Tag Manager |
| `i` (vim nav) | Edit the transcript directly |
| `Ctrl + S` | Save the focused editor |
| `:w` / `:wq` / `:q` | Save / save-and-leave / leave (vim editors) |
| `Shift + Esc` | Leave the transcript / notes editor back to pane navigation |

Transcripts and notes never auto-save — leaving with unsaved changes always
prompts first.

## Vim pane navigation (`interface.vim_nav`)

The three panes — sidebar · list · detail — form a grid you roam with
`h`/`l` (left/right) and `j`/`k` (down/up). Entering a pane lands a visible
cursor immediately.

**Sidebar** — `j`/`k` step through everything: the Library filters, the Tags
filters, the **section headers** (Enter folds/unfolds them), and the **queue**
(its pending items, the pinned active item, and the queue header). On queue
rows, `h`/`l` walk the row's buttons — open · move up · move down · remove —
and on the queue header: fold · failed badge · pause · clear. `l` past a row's
last button steps over to the list.

**List** — `j`/`k` move rows; `k` at the very top steps up into the header
search box (highlighted, not focused — `h`/`l` then roam the header controls,
`Enter` opens dropdowns, `↓`/`Esc` come back).

**Detail** — the open recording is a grid of rows, top to bottom: window
buttons · action buttons · tags · transcript · transcript-view buttons
(Speakers / Summary / Compare / Original / Unedited) · notes. `j`/`k` move
between rows, `h`/`l` along a row (h at the leftmost edge returns to the
list), `Enter` presses the button or enters the editor, `Shift + Enter` on the
tags row opens the Tag Manager.

**Escape ladder** — `Esc` steps out one level at a time: editor → detail nav →
list → close the recording.

## Customizing

The global record hotkeys (push-to-talk, in-place dictation, meeting toggle)
are configured under **Settings → Capture**, and work system-wide even when
Phoneme is in the tray.
