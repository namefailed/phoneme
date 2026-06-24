# Integration API (for companion apps)

Phoneme is built to be a **local backend** other local-first apps delegate to — transcription,
chapters, semantic search, RAG, audio clipping, and PKM export. The flagship consumer is
[youtube-note-thing](https://github.com/namefailed/youtube-note-thing). This page is the stable
contract an integrator builds against.

Three surfaces reach the same daemon. Prefer the one that fits your platform:

| Surface | Best for | Cross-platform |
|---------|----------|----------------|
| **CLI `--json`** | shelling out from any language; the most complete surface | ✅ (one binary, all OSes) |
| **REST** (`phoneme-rest`, loopback HTTP) | apps that already speak HTTP; live SSE progress | ✅ |
| **named pipe** (`\\.\pipe\phoneme-daemon` / Unix socket) | the full IPC schema (advanced) | ⚠️ path differs per OS |

## Versioning / compatibility

`GET /api/status` and `phoneme --json show`-style commands come from a daemon whose version is
reported by **`GET /api/status` → `version`** (the crate version, e.g. `"1.8.0"`). Probe it on
connect and gate features on a minimum version rather than assuming a shape. The CLI `--json`
output and the REST JSON are the same daemon DTOs serialized as snake_case; treat unknown fields
as additive and parse defensively (ignore fields you don't know).

## CLI (`phoneme --json …`)

| Need | Command | Output |
|------|---------|--------|
| Is Phoneme present? | `phoneme --json list --limit 1` | exits 0 if the daemon answered |
| Import a URL (YouTube) | `phoneme import <url>` | downloads via yt-dlp, prints the recording id |
| Import a local file | `phoneme import <path>` | prints the recording id |
| Full recording metadata | `phoneme --json show <id>` | `Recording` DTO — status, title, summary, model, `detected_language`, `duration_ms`, `mean_confidence`, **`entities[]`**, **`tasks[]`**, `speaker_names[]`, … (entities/tasks/speaker_names are populated on `show`) |
| Timed segments | `phoneme --json show <id> --segments` | `[{start_ms,end_ms,text,speaker}]` (empty until transcribed) |
| Chapters | `phoneme --json chapters <id> [--show]` | `[{idx,start_ms,end_ms,title,summary}]` (`--show` = view stored, no regenerate) |
| Transcript versions | `phoneme --json versions <id>` | `[{idx,label,model,text,…}]` — the raw-ASR→step→live chain |
| Search | `phoneme --json search <q> --limit N` | NDJSON of `{recording, score}` |
| More like this | `phoneme --json search --like <id> --limit N` | NDJSON of `{recording, score}` |

## REST (`phoneme-rest`, loopback only)

`GET` reads, `POST` writes. JSON in, JSON out; errors are `{"error": "…"}` with a status code.

| Method | Path | Daemon request |
|--------|------|----------------|
| GET | `/api/status` (and `/api/health`) | `DaemonStatus` (includes `version`) |
| GET | `/api/recordings` · `/api/recordings/{id}` | `ListRecordings` · `GetRecording` |
| GET | `/api/recordings/{id}/segments` · `/words` · `/chapters` | `GetSegments` · `GetWords` · `GetChapters` |
| GET | `/api/recordings/{id}/versions` | `ListTranscriptVersions` *(cross-platform alternative to the pipe)* |
| POST | `/api/recordings/{id}/clip` `{start_ms,end_ms[,out_path]}` | `ExportClip` → `{path}` |
| GET | `/api/recordings/{id}/similar` · `/api/search?q=` | `MoreLikeThis` · `SemanticSearch` |
| GET | `/api/tags` · `/api/recordings/{id}/tags` · `/api/queue` | `ListTags` · `TagsFor` · `ListQueue` |
| POST | `/api/recordings/{id}/{title,favorite,pinned,tags,cleanup,summary}` | the matching mutation |
| GET | `/api/events` | `SubscribeEvents` (SSE — live pipeline progress) |

## PKM export

Don't re-implement export adapters — Phoneme runs **hooks** on `RecordingStopped` (and on demand
via `RefireHook`). Templates in `hooks/`: `to-markdown-daily`, `to-org-journal`, `to-denote`,
`to-timestamped-note`, `to-todoist`, `to-webhook`, `to-clipboard`, … Each gets the recording
payload on stdin as JSON.

## Known gaps (not yet on REST/CLI — pipe-only)

- **Import over HTTP** — `POST /api/import {url}` is not yet exposed; URL import is CLI-only
  (`phoneme import <url>`), because the yt-dlp pipeline lives in the CLI. Shell out for now.
- **RAG `Ask`** — streaming Q&A is named-pipe-only (`Ask` request → `AskActivity` events).
- These are tracked as follow-ups; the CLI/REST surfaces above cover the common path.
