# ⌨️ Keyboard Navigation

Phoneme is fully drivable from the keyboard. Press **`?`** anywhere for the
built-in cheat sheet (it shows exactly what's active for your settings).

Everything in **Global**, **Recordings list**, **Bulk actions**, and **Open
recording** below works for everyone, no setup required. Moving _within_ a region
(the sidebar's filters, a recording's buttons, the header) is **`Tab` /
`Shift+Tab`** out of the box. On top of that are two **opt-in** navigation layers,
either or both:

- **Arrow-key navigation** (`interface.arrow_nav`, **Settings → Appearance →
  Arrow-key navigation**) — `←`/`→`/`↑`/`↓` move between and within panes, `Enter`
  opens, `Esc` steps out. The friendly, no-vim-knowledge option.
- **Vim navigation** (`interface.vim_nav`) — the `h`/`l`/`j`/`k`/`gg`/`G`/`dd`/`zz`
  motion layer for vim users. Adds extras the arrow layer doesn't (`dd`, `zz`,
  `x b`/`x /`, ±5s waveform scrub).

Both are independent of the transcript editor's own vim mode (`editor.vim_mode`).

**In modals & popups** (the Re-run / Models picker, Doctor, confirmations, the Tag
Manager, …): with either layer on, `h`/`l`/`j`/`k` and the arrow keys move a cursor
across the dialog's controls, `Enter` activates the highlighted one (a button fires,
a field opens for typing), and `Esc` closes the dialog — the same idiom as the rest
of the app, so no dialog traps you on the mouse. With **neither** layer on, `Tab` /
`Shift+Tab` walk the dialog's controls and `Esc` still closes it, so popups are fully
keyboard-operable for everyone. (Settings is the one exception: it's not
keyboard-navigated yet, but `Esc` leaves it.)

## Global

| Keys | Action |
|------|--------|
| `/` | Focus the search box |
| `?` | Open the shortcut cheat sheet |
| `g` then `l` | Go to the Library |
| `g` then `s` | Go to Settings |
| `g` then `d` | Put the keyboard in the open recording's pane |
| `g` then `D` | Go to the Doctor |
| `g` then `A` | Toggle the AI-activity panel (the floating brain/FAB; its log persists across restarts) |
| `g` then `/` | Highlight the search bar (`h`/`l` then roam the header) |
| `g` then `T` | Open the Tag Manager (quick popup) |
| `g` then `P` | Settings → Managers → Profiles |
| `g` then `S` | Settings → Managers → Saved searches |
| `x` then `b` | Toggle the sidebar (vim twin of `Ctrl + B`) |
| `x` then `/` | Toggle the top (search/header) bar (vim twin of `Ctrl + /`) |
| `Ctrl + ,` | Open Settings |
| `Ctrl + B` | Toggle the sidebar |
| `Ctrl + \` | Toggle the detail pane |
| `Ctrl + D` | Toggle the detail pane (alias of `Ctrl + \`) |
| `Ctrl + /` | Hide / show the top (search/header) bar |
| `Ctrl + =` / `Ctrl + -` / `Ctrl + 0` | Zoom the recordings list bigger / smaller / reset |
| `Ctrl + Shift + =` / `Ctrl + Shift + -` | Bump the **global UI text size** (the `interface.ui_font_size` you set in Settings → Appearance) — distinct from the list-only zoom above |
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
| `\` | **Split mode**: with a recording open, splits with the cursor row; with exactly two selected, opens both — two full panes (independent editing, draggable divider, double-click = 50/50). `Esc` closes the second pane. On an open **meeting** (merged view) it opens the **dual timeline** — both tracks as synced, clickable timelines; `Esc` returns to the merged view |
| `Delete` | Delete the selection — every selected recording, or the open one if nothing is multi-selected (with Undo). The no-vim twin of `dd` |
| `d` `d` (vim nav) | Delete the selection — every selected recording, or the focused one if nothing is multi-selected (with Undo) |
| `g g` / `G` (vim nav) | Jump to the first / last row |
| `z z` (vim nav) | Center the list on the cursor row |
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
| `f` | Zen toggle: a full-window recording — or, with nothing open, a full-window list (sidebar + top bar slide away; `Esc` steps back; `/` peeks the search bar) |
| `t` | Focus the add-tag box (`j`/`k` browse suggestions · `Enter` adds) |
| `Shift + T` | Open the Tag Manager |
| `i` (vim nav) | Edit the transcript directly |
| `Ctrl + S` | Save the focused editor |
| `:w` / `:wq` / `:q` | Save / save-and-leave / leave (vim editors) |
| `Shift + Esc` | Leave the transcript / notes editor back to pane navigation |

Transcripts and notes never auto-save — leaving with unsaved changes always
prompts first.

**In the transcript editor** two rough edges are gone:

- **The wheel scrolls the pane when the editor can't.** The transcript editor
  (CodeMirror) used to trap the mouse wheel: if its own text already fit, or you
  were at the top/bottom of it, scrolling over the editor did nothing and the
  page froze. Now the wheel scrolls the editor's own content whenever it has more
  to show, and **hands the scroll back to the detail pane** the moment the editor
  reaches its boundary — so the page never gets stuck under the editor.
- **Focusing the editor no longer re-centers the transcript.** Keyboard-focusing
  the transcript — the `e` (`edit`) action, or stepping the cursor onto the
  editor cell in arrow/vim navigation — keeps the focus behavior but **no longer
  yanks the transcript to the middle of the pane**. The view stays where it was;
  only the abrupt jump is gone.

## Arrow-key navigation (`interface.arrow_nav`)

The friendly counterpart to vim navigation — same idea, but driven entirely by
the arrow keys, so there's nothing to learn. Turn it on at **Settings →
Appearance → Arrow-key navigation** (or in the first-run wizard).

| Keys | Action |
|------|--------|
| `←` `→` | Move focus between the sidebar, list, and detail panes |
| `↑` `↓` | Move within the list · sidebar filters · detail rows |
| `↑` at the top of the list | Rise into the header controls (`↓` comes back) |
| `←` `→` (in the header) | Move across the header controls |
| `Enter` | Open / activate the focused row, button, or dropdown |
| `Esc` | Step back out a level |

It drives the **same** pane/grid cursor the vim layer uses, so the two can be on
at once (arrows _and_ `h`/`l`/`j`/`k` both work). The bare `g`-chords —
`g b` (sidebar), `g 1`/`g 2` (split panes), `g d` (detail), `g l` (list) — jump
straight to a place and work whether or not either layer is on. The vim-only
extras (`dd`, `zz`, `gg`/`G`, `x b`/`x /`, ±5s waveform scrub) have no arrow
equivalent — turn on vim navigation if you want them.

## Vim pane navigation (`interface.vim_nav`)

The three panes — sidebar · list · detail — form a grid you roam with
`h`/`l` (left/right) and `j`/`k` (down/up). Entering a pane lands a visible
cursor immediately.

**Each pane remembers where you were.** Leave the list, sidebar, header strip,
or a recording's detail pane and come back, and the cursor returns to the row /
control you left — so bouncing between panes never loses your place. The first
time you enter a pane it lands on a sensible default (the list's first row, the
sidebar's top, the header's search box); after that it restores. The detail
pane remembers **per recording**, so opening a different one starts fresh on its
transcript rather than a stale cell.

**Sidebar** — `j`/`k` step through everything: the Library filters, the Tags
filters, the **section headers** (Enter folds/unfolds them), and the **queue**
(its pending items, the pinned active item, and the queue header). On queue
rows, `h`/`l` walk the row's controls — open · the ▲/▼ **move pair** · remove —
and on the queue header: fold · failed badge · pause · clear. The ▲/▼ pair is
**one** `h`/`l` stop: land on it, then `j`/`k` pick move-up vs move-down. `l`
past a row's last control steps over to the list. `gg`/`G` here jump to the top
/ bottom of the **current section** (Library / Tags / Queue), not the whole
sidebar.

**List** — `j`/`k` move rows; `gg`/`G` jump to the first / last row; `k` at the
very top steps up into the header search box (highlighted, not focused — `h`/`l`
then roam the header controls, `Enter` opens dropdowns, `↓`/`Esc` come back).

**Header roving** — the cursor is **sticky** and follows your clicks. Clicking a
header control places the roving cursor on it. `Enter` on a plain control
**fires it and keeps the cursor on it** (so after sorting or toggling you keep
roaming with `h`/`l`); `Enter` on a date field opens its calendar. `Esc` from a
header **input** (search box or a date filter) returns the cursor **to** that
control — a *second* `Esc` from roving then drops to the list.

**Detail** — the open recording is a **2-D grid**: `j`/`k` move down/up between
rows, `h`/`l` move within a row (`h` at the start of a row returns to the list),
and `Enter` edits the box / fires the button / opens the dropdown under the
cursor. Top to bottom the rows are:

1. **Title** — editable; `Enter` to rename.
2. **Waveform player** — `Enter` enters **scrub mode**: `h`/`l` nudge ±1 s,
   `H`/`L` jump ±5 s, `Space` plays/pauses, and `Esc` (or `j`/`k`) leaves it.
3. **Action row** — the recording's buttons.
4. **Applied tag chips** — the tags already on the recording.
5. **Tag input + controls** — add a tag (`Shift + Enter` opens the Tag Manager).
6. **Pending tag suggestions** — each carries its own **✓** (apply) / **✗**
   (dismiss).
7. **Transcript** — `Enter`/`i` to edit.
8. **Speakers / Views / Versions** — the in-box buttons.
9. **Notes** — `Enter` to edit; `Shift + Esc` leaves it.
10. **Pipeline** — opens the pipeline.

**Dropdowns** — the detail dropdowns (Speed · Export · Views · Versions ·
Pipeline) and the bulk bar's Tag / Export are fully keyboard-driven: `Enter`
opens, `j`/`k` cycle the options, `Enter` picks, and `Esc`/`h`/`l` close.

**Split view** — with two recording panes open (`\`), `h`/`l` cross between
them at a row's edge: `h` from a row's left edge steps into the left pane, and
`l` from its right edge into the right pane (mid-row they keep moving within the
row, as usual). `g 1` / `g 2` jump straight to the left / right pane.

**Escape ladder** — `Esc` steps out one level at a time: editor → detail nav →
list → close the recording. From the **Notes** (or transcript) editor, use
`Shift + Esc` to step back to pane navigation.

## Customizing

The global record hotkeys (push-to-talk, in-place dictation, meeting toggle)
are configured under **Settings → Capture**, and work system-wide even when
Phoneme is in the tray.
