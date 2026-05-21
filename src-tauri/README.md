# phoneme-tray

Tauri 2 desktop shell for [Phoneme](../README.md). System tray + recordings GUI
that talks to `phoneme-daemon` over the same named-pipe IPC as the CLI.

## Modules

| Module | Responsibility |
|---|---|
| `bridge` | `NamedPipeTransport` wrapper with auto-reconnect |
| `commands` | `#[tauri::command]` handlers (list/get/delete/record_*/replay/refire/update_transcript/daemon_status) |
| `tray` | System tray icon + menu + state-driven icon/tooltip swaps |
| `events` | Background task that subscribes to `DaemonEvent` and re-emits to the frontend via Tauri events |

## Build

```bash
# One-time install of Tauri CLI
cargo install tauri-cli --version ^2

# Dev (hot-reload frontend, native window)
cargo tauri dev

# Release MSI
cargo tauri build
```

The frontend lives in `../frontend/` (Vite + vanilla TS). Tauri's
`beforeDevCommand` / `beforeBuildCommand` invoke `pnpm` there.

## Tray menu

| Item | Behavior |
|---|---|
| ● Record | Emits `menu:record` to the frontend (which calls `record_start`) |
| ◼ Stop | Emits `menu:stop` (frontend calls `record_stop`) |
| Show window | Shows + focuses the main window |
| Doctor | Shows the window and emits `nav:doctor` |
| Settings | Shows the window and emits `nav:settings` |
| Quit | Exits the app |

Left-click on the tray icon toggles the window. Icon + tooltip update from
`events.rs` as `DaemonEvent`s arrive (idle → recording → transcribing →
back to idle, or → error on llama/hook failures).

## Frontend bridge

All daemon IPC goes through the bridge:

```
Frontend (TS)  --invoke("list_recordings")-->  Tauri Cmd  --Request-->  Daemon
                                                                          |
Frontend       <--Tauri event "daemon-event"--  Tray events  <--Event----+
```

The frontend has type definitions for every `Request` payload and `DaemonEvent`
shape in `../frontend/src/services/ipc.ts` and `events.ts`.
