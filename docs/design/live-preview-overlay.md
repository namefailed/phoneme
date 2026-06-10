# Live preview — smoothness & system-wide overlay

Status: implemented (opt-in, off by default). This note records the design of the
real-time "live preview" feature and the always-on-top desktop overlay, plus the
open UX decisions left for the user.

## What "live preview" is

While a recording (or meeting) is in progress, the daemon periodically
re-transcribes the most recent slice of captured audio and emits a
`transcription_partial { id, text }` event. The frontend shows this as a live,
forward-growing caption. It is a **preview** only — the authoritative transcript
is still produced by the normal post-stop pipeline. Everything here is gated on
`recording.streaming_preview` (default `false`); when off, no preview task runs
and behavior is byte-for-byte the historical one.

Two surfaces show the same partial stream:

1. **In-app ticker** — a floating card pinned to the bottom-center of the main
   window (`frontend/src/components/HeaderBar.ts`).
2. **System-wide overlay** — a separate, frameless, always-on-top window that
   floats the caption over the whole desktop, even when the app window is hidden
   (`frontend/overlay.html` + `frontend/src/overlay.ts`). Gated additionally on
   `interface.preview_overlay` (default `false`).

## Part 1 — smoothness

The jank in the original preview came from three places; all three are addressed
and kept cheap (local-first, no extra network):

- **Emission cadence / cost (daemon, `bin/phoneme-daemon/src/recorder.rs`).**
  - The preview transcribes only the **last `PREVIEW_WINDOW_SAMPLES` (~15 s)** of
    audio each tick, so per-tick cost is constant instead of O(n) growing with
    the take (which was O(n²) over a recording and saturated whisper-server on
    long takes).
  - Ticks are throttled: `interval.set_missed_tick_behavior(Skip)` plus a
    `PREVIEW_MIN_NEW_SAMPLES` (~1 s) gate, so a slow transcription never queues a
    burst and a near-silent gap doesn't spend a transcription.
  - The preview yields to the final pipeline: it only runs a tick if the single
    serial whisper permit (`state.whisper_sem.try_acquire()`) is free right now,
    so it can never starve the authoritative transcription.
  - A **native** preview provider drops the interval to 1 s (no HTTP/file
    overhead); cloud providers keep the longer `PREVIEW_INTERVAL`.
  - **`stitch_preview`** turns the sliding window into a stable, forward-growing
    caption: once the audio window starts sliding it appends only the genuinely
    new tail (longest word-boundary overlap, case-insensitive) instead of letting
    the caption's start "rewind" each tick. Pure function, unit-tested.

- **Render cadence (frontend `HeaderBar.ts` and `overlay.ts`).** Partials are
  coalesced with a 150 ms throttle that always renders the **latest** text (not a
  debounce that reset on every event and only added lag). The text box is clamped
  to ~2 lines and pinned to its bottom so the newest words stay visible while
  older text scrolls up; a short opacity transition softens each swap.

- **Previews for meetings, not just single recordings.** Previously the preview
  loop only ran from the single-recording `start()` path, because a meeting's
  recorders live inside `ActiveMeeting`, not `self.handle` — so meetings emitted
  no partials at all even though the frontend was ready to show them. Fixed by
  introducing a cheap, cloneable `SnapshotHandle`
  (`crates/phoneme-audio/src/recorder.rs`) that can read a live recorder's
  trailing audio window without owning the `Recorder`. `start_preview` now takes
  a `SnapshotHandle`, and the meeting path hands it the **mic** track's handle
  (the dense local voice; the system track is the sparse remote side). The
  preview is torn down in `stop_meeting` and the meeting `cancel` branch, exactly
  like the single path.

## Part 2 — system-wide overlay architecture

### Window creation

The overlay is a second Tauri `WebviewWindow` (label `preview-overlay`) created
**at runtime** rather than declared in `tauri.conf.json`, because it should only
exist when the user opts in. `src-tauri/src/overlay.rs` owns this:

- `ensure(app)` builds it (idempotent): frameless (`decorations(false)`),
  `transparent(true)`, `always_on_top(true)`, `skip_taskbar(true)`,
  `resizable(false)`, `focused(false)`, and **`visible(false)`** — it starts
  hidden so the first recording can reveal it with zero cold-start lag.
- `destroy(app)` closes it; `sync(app, enabled)` reconciles existence against the
  setting.
- It is pre-created (hidden) at startup when the setting is on (`lib.rs` setup),
  and reconciled on every config save / profile switch (`commands::apply_config`
  → `overlay::sync`). Turning the setting off closes the window so no invisible
  webview lingers.

`overlay.html` is a standalone page that mounts only the caption card (no app
shell/router), so the window stays tiny and cheap. **Build wiring:** because Vite
is otherwise single-entry, `frontend/vite.config.ts` now lists both `index.html`
and `overlay.html` as Rollup inputs — without that, `overlay.html` would be
omitted from `dist/` and the window would 404. The same `overlay.html` relative
URL works for both the dev server and the bundled `frontendDist`.

`src-tauri/capabilities/default.json` adds `preview-overlay` to the window
allowlist and grants it `core:window:allow-set-always-on-top`,
`allow-set-position`, and `allow-start-dragging` (plus the existing show/hide), so
the overlay's JS can manage itself.

### Event forwarding

No bespoke forwarding was needed. `src-tauri/src/events.rs` already re-emits every
daemon event with `app.emit("daemon-event", …)`, and Tauri's `Emitter::emit`
broadcasts to **all** webviews. So the overlay receives the identical
`recording_started` / `transcription_partial` / `recording_stopped` stream the
main window does the instant it exists. `recording_started` fires for both single
recordings and meeting tracks, so **auto-show works for both** with no extra code.

### Visibility lifecycle

Rust never forces the overlay visible; `overlay.ts` drives it from the event
stream:

- `recording_started` → clear text, `win.show()`, re-assert always-on-top.
- `transcription_partial` → throttled caption render (unless the user manually
  hid it; the hide is respected until the next start).
- `recording_stopped` / `_cancelled` / `_deleted` → keep the last caption briefly,
  then dim (2.5 s), then hide (4 s). The generous delay covers a meeting's two
  tracks (a stop on one shouldn't tear down the overlay while the other is live);
  a fresh start/partial cancels it by re-showing.

A `set_overlay` Tauri command (`commands.rs`) exposes explicit `show` / `hide` /
`move` for the Settings "Preview" button (so the user can see and drag the card
without recording) and any future keyboard toggle.

### Position persistence

The whole card is a `data-tauri-drag-region`, so the user drags the overlay
anywhere. Its position is remembered across runs by the already-present
`tauri-plugin-window-state` plugin, which saves/restores geometry per window
label automatically — so no hand-rolled persistence. On first-ever creation (no
saved state) `overlay.rs` places it bottom-center of the current monitor's work
area.

### Settings

`frontend/src/components/SettingsView/SectionPreview.ts` gains a **"System-wide
overlay"** checkbox bound to `interface.preview_overlay`, plus a **"Preview"**
button that briefly shows the overlay so the user can position it. The overlay
controls are disabled unless live preview itself is enabled (the overlay has
nothing to show without partials); turning live preview off also clears the
overlay flag.

## Safe defaults chosen

- Always-on-top: **on** (re-asserted on each show).
- Frameless + transparent: **on** (it's a floating caption, not a window).
- Movable + remembered position: **on** (drag region + window-state plugin).
- Auto-show on record start (single or meeting): **on**.
- Auto-dim then hide after stop: **on** (2.5 s dim / 4 s hide).
- Whole feature: **off by default** (`interface.preview_overlay = false`), and
  itself requires `recording.streaming_preview`.

## Open UX decisions (for the user to confirm)

1. **Click-through vs interactive.** The overlay is currently interactive: you can
   drag it and click its ✕. The alternative is a click-through overlay
   (`set_ignore_cursor_events(true)`) that never intercepts clicks — better as a
   pure HUD, but then it can't be dragged or dismissed by mouse and would need a
   hotkey/Settings to reposition. Decision: keep interactive, or add a
   "click-through (HUD) mode" toggle?

2. **Auto-hide timing.** Dim at 2.5 s, hide at 4 s after stop. Should the caption
   instead **stay** until the next recording (no auto-hide), or be configurable?
   The current generous delay is a heuristic to bridge a meeting's two-track stop;
   a cleaner fix would be for the daemon to emit a single "capture fully idle"
   signal the overlay could key off.

3. **Multi-monitor.** First-run placement targets the overlay's *current* monitor
   (effectively the primary). The window-state plugin then remembers wherever the
   user drags it. Open question: should the overlay follow the monitor that holds
   the foreground/recording app, or always pin to a user-chosen display? Today it
   simply stays where it was last left.

4. **Per-window theme.** The overlay reads `interface.theme` once on load to match
   the app. It does not live-update if the user changes theme while the overlay is
   open; it picks up the new theme on its next (re)creation. Acceptable, or should
   it subscribe to a theme-changed event?
