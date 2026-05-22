# Phoneme — Code Review (2026-05-21)

**Snapshot:** master tip `8f944ff` — Plan 4 complete.  
**Context:** Local-first desktop app, personal use to a few hundred people.
No multi-tenancy. No remote attack surface (named pipe is local-only). Plan 1–4
merged; Plans 5–9 unstarted. Review calibrated accordingly — no speculative
hardening for a scale this app will never reach.

**Methodology:** Synthesised two full independent reviews of the same codebase.
Items appear below only if real evidence was found in the code. Severity was
re-evaluated against actual user impact, not theoretical worst-case.

**Resolution:** All 16 findings were addressed on branch
`fix/code-review-2026-05-21` and merged to master. Two intentional
divergences from the suggested fixes:
- **#9 (RefireHook):** the suggested fix re-enqueued into the pipeline
  queue. That queue always re-transcribes, which would clobber a user's
  manual transcript edit — so RefireHook instead runs the hook in a
  detached task, fixing the IPC-blocking problem while keeping the
  "re-run only the hook" semantic.
- **#14 (exit-code sentinel):** left as-is. On Windows every process has
  an exit code, so `status.code()` is always `Some` and the `-1`
  `unwrap_or` branch is unreachable; making `exit_code` an `Option`
  would ripple through HookResult → pipeline → catalog → DaemonEvent for
  no real-world gain on the target platform.
- **#16 (dead log-rotation config):** deferred to Plan 5/6 as the review
  itself recommended.

---

## Summary

| # | Severity | File | Finding |
|---|---|---|---|
| 1 | **critical** | `hook.rs` | Hook timeout leaks child process |
| 2 | **critical** | `named_pipe.rs` / `record.rs` | Subscribe ACK consumed as event — `phoneme record` always exits with error |
| 3 | **critical** | `queue.rs` | Corrupt inbox file permanently blocks the queue |
| 4 | **critical** | `ipc_handler.rs` | `DeleteRecording` silently ignores catalog-delete failure |
| 5 | **critical** | `ipc_handler.rs` | `Request::Shutdown` is a no-op — daemon ignores stop command |
| 6 | **important** | `catalog.rs` | `ListFilter::search` silently ignored in `Catalog::list()` |
| 7 | **important** | `id.rs` | Unchecked byte-offset slicing panics on any non-18-char `RecordingId` |
| 8 | **important** | `queue_worker.rs` | `claim_next` I/O error kills the worker permanently |
| 9 | **important** | `ipc_handler.rs` | `RefireHook` blocks the IPC connection for up to 30 s |
| 10 | **important** | `pipeline.rs` | `TranscriptionClient` re-created per queued item |
| 11 | **important** | `doctor.rs` | `audio_dir` existence check uses un-expanded path — always shows missing |
| 12 | **nitpick** | `queue.rs` | Dead `entries.remove(0)` |
| 13 | **nitpick** | `id.rs` | Counter starts at 1 — suffix `000` is never produced |
| 14 | **nitpick** | `hook.rs` | Exit code `-1` sentinel used where the field is `Option<i32>` |
| 15 | **nitpick** | `llm_supervisor.rs` | Backoff not reset after a long-healthy run |
| 16 | **nitpick** | `config.rs` | `log_max_size_mb` / `log_max_files` parsed but never used |

**Intentionally omitted (over-engineered for this scale):**
- `PHONEME_DATA_LOCAL` path injection — gate to `#[cfg(test)]`, 2 lines.
  Not a threat model concern on a single-user desktop.
- `Bridge::request` blind retry on mutations — local named pipe never drops
  mid-write in practice. Revisit if a network transport is ever added.
- `SyntheticSource` channel capacity — only matters for integration tests that
  don't exist yet (Plan 3a deferred). Fix when writing those tests.
- `NamedPipeConnection` Framed read/write coupling — "not a correctness bug"
  by the older review's own admission.
- `shellexpand` expanding `$VAR` in hook command — misunderstands the execution
  model. `shlex` splits the command line; PowerShell's `$input`/`$env:` inside
  the script are never touched by shlex.

---

## 1 · CRITICAL — Hook timeout leaks child process

**File:** `crates/phoneme-core/src/hook.rs:52–58`

```rust
// current
Err(_) => {
    return Err(Error::HookTimeout { secs: self.timeout.as_secs() });
    // child is dropped here — Tokio drop on Windows does NOT kill the process
}
```

On Windows, dropping `tokio::process::Child` without calling `start_kill()`
first leaves the process running. Every hook timeout leaks one `powershell.exe`.

**Fix — 3 lines in the `Err(_)` arm of the `timeout(…)` match:**

```rust
Err(_) => {
    let _ = child.start_kill();
    let _ = child.wait().await;
    return Err(Error::HookTimeout { secs: self.timeout.as_secs() });
}
```

---

## 2 · CRITICAL — `phoneme record` always exits with error after subscribing

**Files:** `crates/phoneme-ipc/src/named_pipe.rs:166–188`,
`bin/phoneme-daemon/src/ipc_handler.rs:19–27`

The server sends `Response::Ok({"subscribed":true})` as an ACK on
`SubscribeEvents`. The client (`NamedPipeTransport::subscribe`) immediately
reframes the connection as `DaemonEvent`-typed **without reading the ACK first**.
The ACK JSON line is then deserialised as a `DaemonEvent`, fails (`DaemonEvent`
has no `"status"` tag), and the stream immediately yields `Err`. The `record`
command treats any stream error as fatal and exits.

**Every blocking `phoneme record` call fails immediately after subscribing.**

**Fix — Option A: client drains ACK before reframing**

```rust
// In NamedPipeTransport::subscribe(), after writing the SubscribeEvents request:
{
    let framed = self.framed.as_mut().ok_or(IpcTransportError::Closed)?;
    // Drain the {"subscribed":true} ACK before reframing.
    let _ = framed.next().await;
}
let old = self.framed.take().ok_or(IpcTransportError::Closed)?;
// ... reframe as DaemonEvent
```

**Fix — Option B (recommended): server skips ACK for SubscribeEvents**

```rust
// In ipc_handler.rs::handle_connection — remove the send_response() call
// for SubscribeEvents and go straight into the event streaming loop:
Ok(Some(Request::SubscribeEvents)) => {
    let mut rx = state.events.subscribe();
    loop {
        match rx.recv().await { … }
    }
}
```

Option B removes a protocol step with no consumer value. Simpler, preferred.

---

## 3 · CRITICAL — Corrupt inbox file permanently blocks the queue

**File:** `crates/phoneme-core/src/queue.rs:99–103`

```rust
// current — read JSON before rename
let payload = read_payload(&file).await?;   // propagates Err if file is corrupt
let processing = …;
fs::rename(&file, &processing).await?;      // never reached on parse error
```

`read_json_entries_sorted` returns the oldest file first. A corrupt `.json`
in `pending/` fails to parse on every `claim_next()` call (every 500 ms),
forever. Every valid file behind it is never processed. The queue is
permanently stuck with no user-visible signal beyond log inspection.

**Fix — rename first, then parse:**

```rust
let processing = self.root.join("processing").join(format!("{id_str}.json"));
fs::rename(&file, &processing).await?;
let payload = match read_payload(&processing).await {
    Ok(p) => p,
    Err(e) => {
        // Move to failed/ so it no longer blocks the queue.
        self.finish_failed(&RecordingId::from_str_unchecked(id_str), "corrupt_payload", &e.to_string()).await?;
        return Ok(None);
    }
};
```

---

## 4 · CRITICAL — `DeleteRecording` silently ignores catalog-delete failure

**File:** `bin/phoneme-daemon/src/ipc_handler.rs:129–136`

```rust
// current
let _ = state.catalog.delete(&id).await;          // error silently dropped
if !keep_audio {
    let _ = tokio::fs::remove_file(&r.audio_path).await;  // also dropped
}
state.events.emit(DaemonEvent::RecordingDeleted { id });
Response::Ok(serde_json::Value::Null)
```

If the catalog delete fails, the client receives `Ok`, the WAV is deleted, but
the DB row remains. The catalog now has a permanent entry pointing to a
non-existent file.

**Fix:**

```rust
if let Err(e) = state.catalog.delete(&id).await {
    return Response::Err(IpcError {
        kind: error_to_kind(&e),
        message: format!("catalog delete failed: {e}"),
    });
}
if !keep_audio {
    // Best-effort; the file may already be gone. Log but don't fail.
    if let Err(e) = tokio::fs::remove_file(&r.audio_path).await {
        tracing::warn!(path = %r.audio_path, error = %e, "audio file removal failed");
    }
}
state.events.emit(DaemonEvent::RecordingDeleted { id });
Response::Ok(serde_json::Value::Null)
```

---

## 5 · CRITICAL — `Request::Shutdown` is a no-op

**File:** `bin/phoneme-daemon/src/ipc_handler.rs:288–292`

```rust
// current
Request::Shutdown => {
    tracing::info!("shutdown requested via IPC");
    Response::Ok(serde_json::Value::Null)
    // Actual shutdown coordination wired in Task 11.
}
```

`phoneme daemon stop` calls this, gets `Ok`, and reports success. The daemon
keeps running. "Task 11" was never landed in Plan 4. The `ShutdownCoordinator`
exists in `shutdown.rs` but isn't referenced from `AppState`.

**Fix — 2-line change to `app_state.rs` + 1-line change to handler:**

```rust
// app_state.rs — add field:
pub shutdown: Arc<ShutdownCoordinator>,

// AppState::new() — store it:
Ok(Self {
    …
    shutdown: Arc::new(ShutdownCoordinator::new()),
})

// ipc_handler.rs — wire it:
Request::Shutdown => {
    tracing::info!("shutdown requested via IPC");
    state.shutdown.trigger();
    Response::Ok(serde_json::Value::Null)
}
```

Note: `main.rs` already constructs a `ShutdownCoordinator` independently.
After this change the two coordinators should be unified — pass the one from
`AppState` into `main` rather than constructing a second one.

---

## 6 · IMPORTANT — `ListFilter::search` silently ignored in `Catalog::list()`

**File:** `crates/phoneme-core/src/catalog.rs:147–169`

`list()` reads `filter.status` and `filter.since` but never reads
`filter.search`. `phoneme list --search "foo"` returns all recordings.
The FTS path exists in `Catalog::search()` but is never called from `list()`.
The CLI populates `filter.search` correctly; the field is simply discarded.

**Fix — delegate to `search()` when the field is present:**

```rust
pub async fn list(&self, filter: &ListFilter) -> Result<Vec<Recording>> {
    if let Some(ref q) = filter.search {
        if !q.trim().is_empty() {
            return self.search(q).await;
        }
    }
    // existing WHERE-clause logic unchanged below
    …
}
```

When search is active this ignores `status`/`since` filters. Acceptable for
now — add a combined FTS+filter query only if users request it.

---

## 7 · IMPORTANT — Unchecked byte-offset slicing on `RecordingId` panics on corrupt data

**File:** `crates/phoneme-core/src/id.rs:49–56`

```rust
pub fn file_stem(&self) -> &str { &self.0[9..] }       // panics if len < 9
pub fn day_folder(&self) -> String {
    format!("{}-{}-{}", &self.0[0..4], &self.0[4..6], &self.0[6..8])  // panics if len < 8
}
```

`from_str_unchecked` (used when loading rows from SQLite and inbox files)
accepts any string length. A single corrupt DB row or hand-renamed inbox file
causes a daemon panic. `from_string` (used for CLI/Tauri IPC input) also
accepts any string.

**Fix — add a `debug_assert` on the internal constructor:**

```rust
pub(crate) fn from_str_unchecked(s: &str) -> Self {
    debug_assert_eq!(s.len(), 18, "RecordingId must be 18 chars, got: {s:?}");
    Self(s.to_string())
}
```

**And a validating public constructor for external input:**

```rust
/// Parse a user-supplied id string. Returns `None` if the format is wrong.
pub fn parse(s: impl Into<String>) -> Option<Self> {
    let s = s.into();
    if s.len() == 18
        && s[..8].chars().all(|c| c.is_ascii_digit())
        && s.as_bytes()[8] == b'T'
        && s[9..].chars().all(|c| c.is_ascii_digit())
    {
        Some(Self(s))
    } else {
        None
    }
}
```

Use `RecordingId::parse(id).ok_or(…not_found…)` in Tauri commands in place of
`RecordingId::from_string(id)`.

---

## 8 · IMPORTANT — `claim_next` I/O error permanently kills the queue worker

**File:** `bin/phoneme-daemon/src/queue_worker.rs:24`

```rust
let claimed = state.inbox.claim_next().await?;  // ? propagates — exits worker
```

A transient I/O error (e.g., NTFS journal flush, antivirus scanner holding a
lock) kills the worker task. The daemon stays up but silently stops transcribing
everything. No user-visible signal.

**Fix — retry with backoff, same pattern as LLM errors:**

```rust
let claimed = match state.inbox.claim_next().await {
    Ok(c) => c,
    Err(e) => {
        tracing::warn!(error = %e, ?backoff, "inbox claim failed; retrying");
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.changed() => return Ok(()),
        }
        backoff = (backoff * 2).min(max_backoff);
        continue;
    }
};
```

---

## 9 · IMPORTANT — `RefireHook` blocks the IPC connection for up to 30 s

**File:** `bin/phoneme-daemon/src/ipc_handler.rs:197–247`

`runner.run(&payload).await` is called inline in the IPC request handler. The
hook timeout is 30 s by default. While the hook runs, the entire connection is
held — the Tauri bridge is effectively frozen. Any subsequent `ListRecordings`
from the UI stalls until the hook completes or times out.

**Fix — enqueue into the pipeline queue instead of running inline:**

```rust
Request::RefireHook { id } => match state.catalog.get(&id).await {
    Ok(Some(r)) if r.transcript.is_some() => {
        // Build a payload with the existing transcript and re-enqueue.
        let payload = HookPayload {
            id: r.id.clone(),
            timestamp: r.started_at,
            transcript: r.transcript.clone().unwrap_or_default(),
            audio_path: r.audio_path.clone(),
            duration_ms: r.duration_ms,
            model: r.model.clone().unwrap_or_default(),
            metadata: HookMetadata::current(),
        };
        match state.inbox.enqueue(&payload).await {
            Ok(()) => {
                let _ = state.catalog.update_status(&id, RecordingStatus::HookRunning).await;
                Response::Ok(serde_json::Value::Null)
            }
            Err(e) => Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: e.to_string(),
            }),
        }
    }
    // … NotFound and missing-transcript arms unchanged
}
```

This makes `RefireHook` consistent with `ReplayRecording`, which already enqueues.

---

## 10 · IMPORTANT — `TranscriptionClient` re-created per queued item

**File:** `bin/phoneme-daemon/src/pipeline.rs:22–25`

`TranscriptionClient::new()` calls `reqwest::Client::builder().build()`, which
allocates a fresh connection pool. Called once per transcription item — no TCP
connection is ever reused to the local llama-server.

**Fix — store in `AppState`, construct once:**

```rust
// app_state.rs
pub transcription: TranscriptionClient,

// AppState::new()
let transcription = TranscriptionClient::new(
    config.llm.external_url.clone(),
    Duration::from_secs(config.llm.timeout_secs),
);

// pipeline.rs — replace client construction with:
let transcript = state.transcription.transcribe(&audio_path).await?;
```

---

## 11 · IMPORTANT — `doctor` checks un-expanded path; always reports `audio_dir` missing

**File:** `bin/phoneme/src/commands/doctor.rs:49–54`

```rust
// current
let audio_dir = std::path::Path::new(&cfg.recording.audio_dir);
// cfg.recording.audio_dir = "%USERPROFILE%/Documents/phoneme/audio"
// Path::new("%USERPROFILE%/…").exists() → always false on Windows
```

The `doctor` command always reports `audio_dir` as not found unless the user
has a pre-expanded literal path in their config. This is immediately visible to
any user running `phoneme doctor`.

**Fix — one line:**

```rust
let expanded_cfg = cfg.expanded()?;
let audio_dir = std::path::Path::new(&expanded_cfg.recording.audio_dir);
```

---

## Nitpicks

**#12 — Dead `entries.remove(0)` in `queue.rs:102`.**
Removes index 0 from a local `Vec` that is immediately dropped after the line.
O(n) shift on dead data. Delete the line.

**#13 — `RecordingId` counter starts at 1; suffix `000` never produced.**
`wrapping_add(1)` before `% 1000` means the sequence is `001…999, 000, 001…`.
Cosmetic; no functional impact.

**#14 — Exit code `-1` sentinel in `hook.rs:65`.**
`output.status.code()` returns `Option<i32>` because signal-killed processes
have no exit code. `-1` is a valid exit code on some platforms. The
`hook_exit_code` DB column is already `Option<i32>`. Store `None` for
signal-killed or timed-out processes instead.

**#15 — `llm_supervisor` backoff not reset after a long-healthy run.**
If llama-server runs healthily for 10 minutes then crashes, `backoff` is
already at or near the 60 s maximum. Reset to `RESTART_BACKOFF_INITIAL` when
a process has been running longer than, say, 60 s.

**#16 — `log_max_size_mb` / `log_max_files` config fields are dead.**
`logging.rs` uses `tracing_appender::rolling::daily` with no size or file-count
limits. The config fields are parsed and stored but never read. Logs grow
without bound. Fix when implementing Plan 5/6 (settings + distribution).

---

## Priority order for fixes

1. **#2** — subscribe ACK issue blocks every user of `phoneme record`
2. **#1** — hook timeout orphans a process on every failure
3. **#5** — daemon can't be stopped cleanly via `phoneme daemon stop`
4. **#3** — corrupt inbox permanently freezes transcription
5. **#4** — silent delete leaves phantom catalog rows
6. **#11** — `phoneme doctor` always shows `audio_dir` missing (bad first-run UX)
7. **#6** — `phoneme list --search` silently returns wrong results
8. **#8** — transient I/O error permanently kills the queue worker
9. **#9** — `RefireHook` freezes the Tauri UI for up to 30 s
10. **#7** — `RecordingId` panic on corrupt data (lower trigger likelihood)
11. **#10** — reqwest pool reuse (quick win, ~5 lines)
12. **#12–16** — nitpicks, take at your leisure
