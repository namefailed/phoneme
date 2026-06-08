# 🎨 Phoneme Frontend

Vite + TypeScript + Lit frontend for the [Phoneme](../README.md) Tauri shell (`phoneme-tray`). Beautifully styled with the Catppuccin Mocha theme.

## 🗂️ Layout

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
│   └── theme.css                # Catppuccin Mocha CSS variables
└── components/
    ├── HeaderBar.ts             # search + filter pills + settings cog
    ├── shared/styles.css        # shared bits (pills, dots, status colors)
    └── RecordingsView/
        ├── index.ts             # orchestrator: list + detail + splitter + live updates
        ├── RecordingsList.ts    # multi-column table
        ├── RecordingDetail.ts   # right pane (waveform + transcript editor + actions)
        ├── ActionRow.ts         # play / replay / refire / copy / reveal / delete
        ├── TranscriptEditor.ts  # autosize textarea with Ctrl+S save
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

## ☕ Catppuccin Mocha

`styles/theme.css` defines the CSS variables every component uses
(`--bg-deep`, `--accent`, `--fg-default`, `--ok`/`--warn`/`--err`, etc.).
The dark palette matches the user's editor and rest of the system theme.

