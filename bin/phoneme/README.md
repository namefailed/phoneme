# 💻 phoneme (CLI)

The lightweight, extremely fast command-line interface for Phoneme. 

## 🗂️ Responsibilities

- **IPC Client**: The CLI contains absolutely no business logic. It simply parses your command line arguments using `clap` and sends a JSON `Request` to the background Daemon over the named pipe.
- **Automation**: Because the Daemon handles all the state, the CLI is the perfect tool for binding to global hotkeys (via AutoHotkey or Kanata) or integrating into shell scripts.

Run `phoneme --help` to see all available commands!
