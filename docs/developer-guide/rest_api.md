# Local REST API (`phoneme-rest`)

`phoneme-rest` is an optional, **off-by-default** localhost HTTP bridge over the
running phoneme daemon. It is a *thin* front-end: every REST endpoint maps one
HTTP request to exactly one `phoneme-ipc` `Request`, forwards it to the daemon
over the existing named pipe, and returns the daemon's JSON answer verbatim.
`GET /api/events` streams the daemon's `DaemonEvent` broadcast as
Server-Sent Events. There is no business logic in the bridge — the daemon stays
the single source of truth.

Source: `bin/phoneme-rest/`. Schema for the `[rest_api]` config section:
`crates/phoneme-core/src/config.rs`. The underlying wire contract every endpoint
forwards to is the [IPC Integration Guide](ipc_integration.md); the
[MCP server](mcp_server.md) is the other thin bridge over the same daemon.

---

## Security: loopback is the trust boundary

The server binds **`127.0.0.1` only — never `0.0.0.0`.** Phoneme is a
local-first, single-user app: the daemon's IPC pipe is already owner-only
(see the named-pipe ACL in `crates/phoneme-ipc/src/named_pipe.rs`), and the
bridge preserves that posture by refusing to listen on any non-loopback
interface. Anything that can reach loopback can already drive the daemon through
the `phoneme` CLI, so loopback is the boundary; exposing this surface to a
network would widen it.

There is **no per-user token** — that is by design for a loopback-only,
single-user surface. If you need remote access, terminate it behind an
authenticating reverse proxy on the same host; **do not** change the bind
address.

A *browser* on the same machine is still in scope, though, so the server
defends against the two ways a web page can reach a loopback service. Both
checks run in one middleware (`loopback_guard`) that wraps **every route and
every method uniformly** — a `GET`, an `EventSource`, a `POST`, and a `DELETE`
are all gated the same way, so there is no method or path it skips:

- **DNS rebinding** — a request whose `Host` header is present and not
  `127.0.0.1`/`localhost`/`[::1]` (any port) is refused with `403`. A page that
  rebinds its own hostname to loopback always sends that foreign `Host`.
- **CSRF / cross-origin reads** — a request whose `Origin` header is present and
  not a loopback origin is refused with `403`, *regardless of method*. That
  covers a cross-site `POST`/form submit, but equally a cross-origin `GET` or an
  `EventSource` to `/api/events` — a foreign page can neither read a response
  nor open an SSE stream to exhaust slots. Cross-site requests always carry a
  foreign `Origin`; a `null` (sandboxed/opaque) origin counts as foreign too.

Non-browser local clients (the `phoneme` CLI, `curl`, scripts) send neither
header and are unaffected. Both checks live in `bin/phoneme-rest/src/server.rs`.

---

## Enabling it

The bridge is opt-in. It reads `[rest_api]` from the active config and **refuses
to start** (clean message, non-zero exit) unless `enabled = true`.

```toml
[rest_api]
enabled = true   # default false — the bridge is off until you turn it on
port    = 3737   # default 3737; 127.0.0.1 only (the interface is not configurable)
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `false` | Allow the `phoneme-rest` bridge to run. When `false`, the binary exits with a clear message and a non-zero code. |
| `port` | u16 | `3737` | TCP port bound on `127.0.0.1`. Only the loopback interface is ever bound. |

Then start the bridge (the daemon must also be running — start it with
`phoneme daemon start` if it isn't):

```sh
phoneme-rest
# phoneme-rest listening on http://127.0.0.1:3737 (loopback only) — forwarding to daemon pipe 'phoneme-daemon'
```

The bridge **never auto-spawns** the daemon. If the daemon is down, requests
return `503` (see error mapping) rather than silently starting one — the
observe-only posture, matching the CLI's read-only commands.

---

## Endpoints

All endpoints live under `/api`. Each maps to one daemon `Request`:

Each endpoint still maps one HTTP request to exactly one `phoneme-ipc`
`Request`. Mutating endpoints carry a small JSON body where the underlying
`Request` needs a value; everything else is path/query only.

### Read

| Method | Path | Daemon `Request` | Notes |
|--------|------|------------------|-------|
| `GET`  | `/api/health` | `DaemonStatus` | `200 {"status":"ok"}` when the daemon answered; `503` otherwise. |
| `GET`  | `/api/status` | `DaemonStatus` | The daemon's liveness + identity probe (pid, version, whisper ports). |
| `GET`  | `/api/recordings` | `ListRecordings` | Query params: `limit` (u32), `offset` (u32), `kind` (`single`\|`meeting`; anything else = all). |
| `GET`  | `/api/recordings/{id}` | `GetRecording` | `id` must be the canonical 18-char id; a malformed id is `400` and is never forwarded. |
| `GET`  | `/api/recordings/{id}/segments` | `GetSegments` | Transcript segments in timeline order (may be an empty array — a normal state). |
| `GET`  | `/api/recordings/{id}/words` | `GetWords` | The per-word layer beneath `segments` (word seek, confidence); may be empty. |
| `GET`  | `/api/recordings/{id}/chapters` | `GetChapters` | Auto-chapters in chronological order (`start_ms`/`end_ms`, `title`, optional `summary`); may be an empty array — a normal state when the recording has no timing to chapter or the auto-chapter step never ran. |
| `GET`  | `/api/recordings/{id}/similar` | `MoreLikeThis` | "More like this" from the recording's stored vectors. Query param: `limit` (usize, default `20`). |
| `GET`  | `/api/recordings/{id}/tags` | `TagsFor` | The tags attached to one recording. |
| `GET`  | `/api/tags` | `ListTags` | Tags attached to at least one recording. |
| `GET`  | `/api/queue` | `ListQueue` | The transcription pipeline queue (processing first, then pending). |
| `GET`  | `/api/search` | `SemanticSearch` | Query params: `q` (string), `limit` (usize, default `20`). |
| `GET`  | `/api/events` | `SubscribeEvents` | Server-Sent Events; see below. |

### Mutate

Like every route, a `POST`/`DELETE` carrying a foreign `Origin` (or a non-loopback
`Host`) is refused with `403` (CSRF / rebinding; see the security section — the
guard is uniform, not POST-only). All id-bearing routes reject a malformed
`{id}` with `400` before any IPC is sent.

| Method | Path | Daemon `Request` | Body / notes |
|--------|------|------------------|--------------|
| `POST`   | `/api/record/start` | `RecordStart` | No body. Starts a `hold`-mode recording (stop is explicit; dictation/in-place is not exposed). |
| `POST`   | `/api/record/stop` | `RecordStop` | No body. Stops and finalizes the active recording. |
| `POST`   | `/api/meeting/start` | `StartMeeting` | No body. Starts a dual-track meeting recording. |
| `POST`   | `/api/meeting/stop` | `StopMeeting` | No body. Stops and finalizes the active meeting. |
| `POST`   | `/api/recordings/{id}/title` | `SetRecordingTitle` | `{"title":"…"}` to set; `{}` or `{"title":null}` to clear back to auto. |
| `POST`   | `/api/recordings/{id}/favorite` | `SetFavorite` | `{"favorite":true|false}`. |
| `POST`   | `/api/recordings/{id}/pinned` | `SetPinned` | `{"pinned":true|false}`. Pinned recordings sort to the top of the library. |
| `POST`   | `/api/recordings/{id}/tags` | `AttachTag` | `{"tag_id":<id>}` — attach an existing tag. |
| `DELETE` | `/api/recordings/{id}/tags/{tag_id}` | `DetachTag` | No body. Detach a tag from a recording. |
| `POST`   | `/api/recordings/{id}/cleanup` | `RerunCleanup` | No body. Re-runs the LLM cleanup step against the stored original transcript (configured provider/model/prompt; the per-run overrides are not exposed over REST). |
| `POST`   | `/api/recordings/{id}/summary` | `RerunSummary` | No body. Generates/regenerates the LLM summary of the current transcript. |

Response bodies are the daemon's JSON values passed straight through (see the
per-`Request` documentation in `crates/phoneme-ipc/src/schema.rs` for each
shape). Errors are returned as `{"error":"<message>"}` with the status below.

> **Scope.** The REST surface maps a high-value subset of the daemon's IPC
> `Request` enum — the read/query, tag, single edit, re-run, and recording/
> meeting-control variants a local automation client needs. It deliberately does
> **not** expose every variant (speaker-correction, named-voice management,
> saved searches, queue reordering, doctor/rebuild, config reload, …). Add a
> route here — mirroring the handler/error shape — when a REST consumer needs
> one; the mapping is a thin translation in `bin/phoneme-rest/src/request_map.rs`.

### Error → status mapping

A request can fail in two ways, mapped to different HTTP status families:

| Condition | HTTP status |
|-----------|-------------|
| Daemon unreachable / pipe closed mid-request (transport error) | `503 Service Unavailable` |
| Malformed `{id}` path segment (never reaches the daemon) | `400 Bad Request` |
| Daemon `not_found` | `404 Not Found` |
| Daemon `already_recording` / `not_recording` / `invalid_config` | `400 Bad Request` |
| Daemon `whisper_unreachable` / `whisper_timeout` / `daemon_not_running` / `shutting_down` | `503 Service Unavailable` |
| Daemon `hook_failed` / `pipe_in_use` / `io` / `internal` | `500 Internal Server Error` |

---

## Server-Sent Events: `GET /api/events`

`GET /api/events` opens a dedicated subscription to the daemon's event broadcast
and forwards each `DaemonEvent` as one SSE `data:` line carrying the serialized
event JSON — the same line `phoneme watch` prints, framed for
`text/event-stream`. A browser client can therefore:

```js
const es = new EventSource("http://127.0.0.1:3737/api/events");
es.onmessage = (e) => {
  const event = JSON.parse(e.data); // { "event": "transcription_done", "id": "...", ... }
  console.log(event.event, event);
};
```

A keep-alive comment is sent every 15s so idle connections don't time out
between events.

**Disconnect handling.** The stream ends cleanly on either side hanging up:

- *Daemon side* — the underlying pipe stream yields end-of-stream or an error;
  the SSE stream stops and the HTTP response closes. Per the IPC broadcast
  contract, a subscriber that the daemon disconnects (e.g. for lagging) should
  reconnect and re-fetch state (`GET /api/recordings`, `GET /api/status`) rather
  than assume continuity.
- *Client side* — dropping the `EventSource` drops the bridge's pipe
  connection; the daemon detects the closed pipe and removes the subscriber. No
  explicit unsubscribe is needed.

---

## Connect strategy

The bridge opens **one short-lived named-pipe connection per request**: connect,
send the one `Request`, read the one `Response`, drop the connection. There is
no pool. The IPC protocol is strictly one-response-per-request and ordered on a
connection, so a shared connection would have to serialize concurrent HTTP
requests behind a lock anyway; a fresh connection per request lets the daemon's
accept loop handle them concurrently. Local pipe connect is sub-millisecond and
the daemon re-arms its listener immediately, so connection churn is not a
bottleneck for a localhost surface. This mirrors the `phoneme` CLI, which also
connects per invocation. `GET /api/events` uses its own separate connection
because `SubscribeEvents` turns a connection one-way.

---

## `curl` examples

```sh
# Is the daemon reachable through the bridge?
curl -s http://127.0.0.1:3737/api/health
# {"status":"ok"}

# Daemon status (pid, version, whisper ports)
curl -s http://127.0.0.1:3737/api/status

# List the 10 most recent recordings
curl -s 'http://127.0.0.1:3737/api/recordings?limit=10'

# Only meeting tracks, second page of 20
curl -s 'http://127.0.0.1:3737/api/recordings?kind=meeting&limit=20&offset=20'

# One recording, its transcript segments, its per-word layer, and its chapters
curl -s http://127.0.0.1:3737/api/recordings/20260519T143500042
curl -s http://127.0.0.1:3737/api/recordings/20260519T143500042/segments
curl -s http://127.0.0.1:3737/api/recordings/20260519T143500042/words
curl -s http://127.0.0.1:3737/api/recordings/20260519T143500042/chapters

# Semantic search, and "more like this" from one recording
curl -s 'http://127.0.0.1:3737/api/search?q=quarterly%20planning&limit=5'
curl -s 'http://127.0.0.1:3737/api/recordings/20260519T143500042/similar?limit=5'

# Tags: list all, list one recording's, attach, detach
curl -s http://127.0.0.1:3737/api/tags
curl -s http://127.0.0.1:3737/api/recordings/20260519T143500042/tags
curl -s -X POST http://127.0.0.1:3737/api/recordings/20260519T143500042/tags \
  -H 'content-type: application/json' -d '{"tag_id":7}'
curl -s -X DELETE http://127.0.0.1:3737/api/recordings/20260519T143500042/tags/7

# Set a title (or clear it), star it
curl -s -X POST http://127.0.0.1:3737/api/recordings/20260519T143500042/title \
  -H 'content-type: application/json' -d '{"title":"Quarterly review"}'
curl -s -X POST http://127.0.0.1:3737/api/recordings/20260519T143500042/favorite \
  -H 'content-type: application/json' -d '{"favorite":true}'
curl -s -X POST http://127.0.0.1:3737/api/recordings/20260519T143500042/pinned \
  -H 'content-type: application/json' -d '{"pinned":true}'

# Re-run cleanup / summary on the stored transcript
curl -s -X POST http://127.0.0.1:3737/api/recordings/20260519T143500042/cleanup
curl -s -X POST http://127.0.0.1:3737/api/recordings/20260519T143500042/summary

# Inspect the pipeline queue
curl -s http://127.0.0.1:3737/api/queue

# Start / stop a recording
curl -s -X POST http://127.0.0.1:3737/api/record/start
curl -s -X POST http://127.0.0.1:3737/api/record/stop

# Start / stop a meeting (dual-track)
curl -s -X POST http://127.0.0.1:3737/api/meeting/start
curl -s -X POST http://127.0.0.1:3737/api/meeting/stop

# Live event stream (Ctrl-C to stop)
curl -N http://127.0.0.1:3737/api/events
```

---

## Relationship to the `Transport` trait (`HttpTransport`)

The roadmap notes an `HttpTransport` impl of the `phoneme-ipc` `Transport` trait
so external clients can reuse the same typed `Request`/`Response`/`DaemonEvent`
surface over HTTP. That client-side transport (an HTTP/SSE client wrapping a
base URL) is **deferred** — it is a separate, non-trivial deliverable (it needs
an HTTP client and SSE parsing) and the *server* is the deliverable here. When
added, it belongs inside `bin/phoneme-rest` as a client helper, not in
`phoneme-ipc`, to keep the wire crate transport-neutral.
