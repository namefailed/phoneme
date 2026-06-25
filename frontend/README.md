# 🎨 Phoneme Frontend

Vite + TypeScript + Lit frontend for the [Phoneme](../README.md) Tauri shell (`phoneme-tray`). Styled with Catppuccin Mocha (the default; the app ships 16 themes).

Full documentation: [docs/README.md](../docs/README.md).

## 🗂️ Layout

A representative slice — `RecordingsView/` holds ~55 files after the god-file
split, so only the load-bearing ones are listed:

```
src/
├── App.ts                       # top-level shell (HeaderBar + RecordingsView)
├── main.ts                      # entry: mounts App into #app
├── services/
│   ├── ipc.ts                   # typed wrappers for tauri invoke()
│   └── events.ts                # typed wrappers for tauri event listen()
├── state/
│   └── store.ts                 # tiny observable store
├── styles/
│   ├── reset.css
│   └── theme.css                # CSS variables every theme overrides
└── components/
    ├── HeaderBar.ts             # search + filter pills + settings cog
    ├── ModelPicker.ts           # the scope-first Re-run / Models modal
    ├── shared/styles.css        # shared bits (pills, dots, status colors)
    └── RecordingsView/          # the list + detail pane (~55 files; a sample)
        ├── index.ts             # orchestrator: list + detail + splitter + live updates
        ├── RecordingsList.ts    # multi-column table
        ├── RecordingDetail.ts   # right pane (waveform + transcript editor + actions)
        ├── ActionRow.ts         # play / speed / re-run / export / captions / delete
        ├── TranscriptEditor.ts  # CodeMirror 6 editor (optional vim), explicit save
        ├── ClipExport.ts        # the "Edit audio" trim/cut modal
        ├── WaveformPlayer.ts    # wavesurfer.js wrapper
        ├── Splitter.ts          # drag-to-resize divider
        └── styles.css           # RecordingsView CSS
```

## 💻 Dev

Full-stack development needs **three terminals** (see
[`CONTRIBUTING.md`](../CONTRIBUTING.md) for details):

```bash
# Terminal 1 (repo root) — daemon logs in the foreground
cargo run -p phoneme-daemon -- --foreground

# Terminal 2 (this directory) — Vite hot reload on :5173
pnpm dev

# Terminal 3 (repo root) — Tauri window; start after Vite is up
cargo tauri dev
```

`@tauri-apps/api` calls (`invoke`, `listen`) only work inside the Tauri
runtime. Standalone `pnpm dev` is useful for layout/styling work; end-to-end
testing needs `cargo tauri dev` with Vite already running.

## 🏗️ Build

```bash
pnpm build   # produces dist/, which Tauri then packages
```

The Tauri config (`../src-tauri/tauri.conf.json`) points
`frontendDist` at `../frontend/dist`. Don't move it without updating that.

## 🧪 Type-check

```bash
pnpm type-check
```

`tsc --noEmit` runs over `src/` and `vite.config.ts`. CI should gate on this.

## ☕ Theming

`styles/theme.css` defines the CSS variables every component uses
(`--bg-deep`, `--accent`, `--fg-default`, `--ok`/`--warn`/`--err`, etc.).
Themes are variable overrides on top of that base. The app ships 16 (11 dark, 5
light), picked under Settings → Appearance; Catppuccin Mocha is the default.

