# phoneme — CLI

The command-line client for [Phoneme](../../README.md). A thin IPC client to
`phoneme-daemon`; auto-spawns the daemon if it's not running.

## Usage

```
phoneme --help
phoneme record --oneshot
phoneme list --since 2026-05-19
phoneme show 20260519T143500823
phoneme doctor
phoneme watch
phoneme daemon status
```

See `phoneme <command> --help` for per-command flags.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Generic failure |
| 2 | Usage error |
| 3 | Daemon not reachable |
| 4 | Whisper unreachable / timeout |
| 5 | Hook failed |
| 6 | Invalid config |
| 7 | Not found |

External scripts (Kanata, AHK, GTD integrations) can branch on these without
parsing stderr.

## JSON output

Every command that produces structured output supports `--json` for one-JSON-
per-line output. Pretty-table is the default for interactive use.

## Color

Colored output is enabled when stdout is a TTY and `NO_COLOR` is not set.
Disable with `--no-color`.

## Running tests

```bash
cargo test -p phoneme
```

Insta snapshot tests verify stable command output. Update snapshots with:

```bash
cargo insta review
```

