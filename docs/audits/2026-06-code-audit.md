# June 2026 code audit — consolidated follow-ups

Two line-by-line audits of `feat/v20-queue-columns` (now merged to `master`).
This is the **working backlog**; nothing here is started yet. Items are grouped
by severity and ordered for execution (see ROADMAP → *Audit follow-ups*).

- **Audit #1** — combined module + file-by-file pass (HEAD `008b341`).
- **Audit #2** — net-new pass on the newest code (HEAD `fdb68f9`), explicitly
  excluding audit #1's items.

**Already resolved (do not re-do):** DPAPI at-rest encryption (`secret_crypto.rs`),
masked-config WebView boundary (S-H2), diarization `to_segments` + coalescing,
webhook-URL Settings field, Semantic-Search tab, list-pane fill + scroll-extend,
detail-pane button/rename/focus overhaul. The transcript-diff, saved-searches,
and curated-models features audited **clean**.

---

## High — bugs / correctness / security

| ID | Location | Issue | Fix |
|----|----------|-------|-----|
| A1‑H1 | `RecordingsView/index.ts:366` | Delete key passes a `session:<meeting_id>` selection to `deleteRecording` | Guard the `session:` prefix before the IPC |
| A1‑H2 | `SectionPostProcessing.ts:61,83` | Cloud `/models` fetch sends the masked sentinel as the key → model list silently fails when a key is saved | Route the fetch through the daemon (real key) or skip when masked |
| A1‑H3 | `commands.rs:1624` (`open_file`) | No path allowlist (unlike `reveal_file`) — a compromised WebView can open arbitrary paths | Canonicalize + restrict like `reveal_file` |
| A1‑H4 | `ipc_handler.rs:~1536` | Import enqueue failure leaves an orphaned catalog row (no inbox) | Roll back the catalog row + WAV on enqueue failure |
| A1‑H5 | `pipeline.rs` / `queue_worker.rs` | Whisper transient failure → `finish_failed` then sleeps, never requeues | Requeue or defer `finish_failed` on transient errors |
| A2‑H1 | `whisper_supervisor.rs:109‑110`, preview `:271‑272` | whisper‑server spawned with piped stdout/stderr that is **never read** → pipe fills (~64 KB) → child blocks → hung transcription / false "Whisper timed out" | Drain pipes to log, or `Stdio::null()` |
| A2‑H2 | `transcription.rs:78` | With `native-whisper`: `if let Some(path) = &whisper.model_path` — `model_path` is `String`, not `Option` → won't compile with the feature on | Use `!path.trim().is_empty()` |
| A2‑H3 | `commands.rs:55‑61`, `lib.rs:204‑207` | If the daemon was down at tray launch, `Bridge` stays `None` forever; `start_daemon` spawns but managed state never reconnects → every `forward()` returns `daemon_not_running`. Auto-reconnect comment is wrong | `Arc<Mutex<Option<Bridge>>>` + lazy connect on first `forward()`, or re-`.manage()` after spawn |
| A2‑H4 | `commands.rs:923‑957` (`wizard_download_model`) | Accepts a renderer-supplied URL with no `is_allowed_download_url` check (unlike `wizard_download_file`) | Hardcode model→URL map, or reuse the allowlist + hash check |
| A2‑H5 | `commands.rs:1626‑1628` (`wizard_run_installer`) | Authorizes via `starts_with(temp_dir())` without canonicalize → path tricks could run unintended binaries | Canonicalize both paths; tie to a known downloaded installer hash/name |

## Medium — correctness / perf / UX / tests

| ID | Location | Issue | Fix |
|----|----------|-------|-----|
| A2‑M1 | `commands.rs:952‑962` | Model download creates the file before HTTP success; non‑2xx leaves it; later `metadata().is_ok()` treats it as "already downloaded" | `.part` download, delete on all error paths, validate size, atomic rename |
| A2‑M2 | `logging.rs:34` | Doc says "10 MB × 5 files" but code is `rolling::daily` — no size cap/prune; `log_max_size_mb`/`log_max_files` are dead config | Size-based rotation or prune; wire config |
| A2‑M3 | `config_cmd.rs:53‑104` | `config set`: non-atomic write, no `validate()`, type coercion (`whisper.model 123`→int), ignores `PHONEME_CONFIG` | Mirror `profile_cmd.rs`: validate, atomic temp+rename, resolved path, reload |
| A2‑M4 | `client.rs:24‑42` | `connect()` auto-spawns on failure → `phoneme daemon status` can never report "not running" | Status/diagnostic paths connect without auto-spawn |
| A2‑M5 | `commands/doctor.rs:13‑37` | `--rebuild-catalog`: Shutdown then immediate `remove_file` (Windows lock race); ignores `PHONEME_DATA_LOCAL`; leaves `-wal`/`-shm` | Wait for exit; resolved data dir; delete sidecars/checkpoint |
| A2‑M6 | `recorder.rs:1299‑1313` | `stop_meeting` per-track `update_status`/`enqueue` use `?` — one failure abandons remaining tracks (WAV branch correctly `continue`s) | Best-effort per track |
| A2‑M7 | `pipeline.rs:88‑97,546‑551` | Model override: `wait_for_whisper_ready` can pass on the old server before kill+respawn; restore + permit drop don't await readiness | Wait for health after override and after restore |
| A2‑M8 | `main.rs:63‑90`, `ipc_handler` ReembedAll/ReloadConfig | Backfill/ReembedAll hold `embedder.read()` for minutes; ReloadConfig needs `write()` → config save / semantic reload blocked; serial Tauri IPC stalls UI | `spawn_blocking` embed loop; don't hold the read lock across the batch; cancelable backfill |
| A2‑M9 | `named_pipe.rs:211‑220` | Client retries `ERROR_PIPE_BUSY` forever (50 ms, no deadline) | Max attempts / timeout → transport error |
| A2‑M10 | `source.rs:318‑360` | CPAL capture only F32/I16; U16 devices fail though `decode.rs` handles all | Add U16 conversion path |
| A2‑M11 | `decode.rs:36` | 6‑hour import cap still allows ~6+ GiB f32 buffer → OOM | Stream to WAV or lower cap |
| A2‑M12 | `queue.rs:212‑223` | `finish_done/failed` write terminal file then delete `processing/`; crash between → duplicate work on recovery | Atomic transition, or recovery ignores IDs already in done/failed |
| A2‑M13 | `config.rs:966‑1012` | `validate()` checks main whisper only — `preview_whisper` cloud keys/URLs unchecked | Apply same validation to preview |
| A2‑M14 | `config.rs:1016‑1036` | `expanded()` misses `preview_whisper.model_path`, `diarization.local_model_path`, `semantic_search.model_dir`, `editor.vimrc_path` | Expand all user path fields |
| A2‑M15 | `doctor.rs:38,82,106` | Doctor ignores `PHONEME_CONFIG`; probes local whisper for cloud providers; model-file check keys on mode not provider | `resolved_config_path()`; branch by provider |
| A2‑M16 | `llm.rs:165` | LLM error bodies read unbounded (transcription truncates) | Cap body size |
| A2‑M17 | `webhook.rs:38` | Webhook failure bodies unbounded in `HookFailed` | Truncate to ~4 KB |
| A2‑M18 | `tray.rs:141` | Profile switch reloads daemon but re-registers only the main hotkey (skips meeting + in-place) | Shared `apply_profile` helper with full side effects |
| A2‑M19 | `capabilities/default.json:5` | `preview-overlay` gets the same broad permissions as main (shell/fs/dialog/updater) | Split a minimal overlay capability set |
| A2‑M20 | `catalog.rs:1053‑1083` | `apply_retention` deletes rows outside a transaction → partial delete + orphaned audio on mid-loop failure | Single transaction or two-phase |
| A2‑M21 | `ipc_handler.rs:534` | `RetranscribeRecording` mutates global config; a concurrent pipeline read sees the wrong provider/model | Per-request override, not ArcSwap mutation |
| A2‑M22 | `ActionRow.test.ts` | Stale vs current RerunForm (3 steps / `.custom-dropdown` vs live 5 steps + `ph-rerun-form`) → CI break / false confidence | Update tests for summarize/all/hook paths |
| A1‑M | various | Cancel queue → distinct status (not `TranscribeFailed`); `delete_audio` retention is dead config; server-side `kind` filter (sparse pages); `column_widths`/`visible_columns` length validation; queue-IPC integration tests; AssemblyAI `utterances.unwrap()` panic; `spawn_blocking` for ONNX embed + diarization; diarizer pipeline cache in `AppState`; typed `readConfig`/`writeConfig`; webhook SSRF guard | (see audit #1) |

## Low — polish

`A2‑L1` `output.rs:49` byte-slice transcript at 60 → UTF-8 panic ·
`A2‑L2` `pipeline.rs:598` cloud backends store `model="unknown"` ·
`A2‑L3` CancelProcessing ignored after cleanup phase ·
`A2‑L4` `record.rs:78` CLI blocking record subscribes after start (race) ·
`A2‑L5` `auto_spawn.rs:43` version-mismatch CLI restarts the GUI's daemon mid-recording ·
`A2‑L6` cleanup/summarize CLI print "complete" while async ·
`A2‑L7` `export.rs:89` full WAV in memory, flattens day folders, omits orphan tags ·
`A2‑L8` import extension list duplicated from `phoneme_audio` ·
`A2‑L9` `list.rs:75` `--tag` uses attached-only tags ·
`A2‑L10` `reconcile.rs:37` mid-capture status swept to TranscribeFailed, no partial WAV recovery ·
`A2‑L11` `app_state.rs:138` possible lost wakeup on override ·
`A2‑L12` `transcription.rs:559` Deepgram `[Speaker 0]` vs local 1-based ·
`A2‑L13` `id.rs:36` suffix wraps at 1000 IDs/sec ·
`A2‑L14` `recorder.rs:169` huge `pre_roll_ms` allocs before validation ·
`A2‑L15` `SectionHotkey.ts:62` meeting Hold/Toggle UI but backend always toggle ·
`A2‑L16` `secret_crypto.rs:39` DPAPI encrypt failure stores plaintext (warned fallback) ·
`A2‑L17` `secret_crypto.rs:56` non-UTF-8 decrypted key → lossy corruption ·
plus audit #1 lows (corrupt `.queue-order` warning log, `native_whisper` hardcodes `en`, tray backlog ignores processing/failed, `HookTest` off-thread, doc drift: CHANGELOG/smoke-test/`building_from_source`/`frontend/README`, populate `docs/screenshots/`, `config validate` CLI, ESLint in CI).

## False positives (do not roadmap)

`catalog.rs` `max_age_days` overflow (unreachable — `u32` days fit chrono) ·
`codec.rs` complete line >8 MiB (accumulation hits the cap first) ·
`recorder.rs` large prepend (edge case — truncated on first live block).

---

## Execution order (when given the go)

1. **Wave 1 — High (correctness/security):** A2‑H1 pipe drain, A2‑H2 native-whisper compile, A2‑H3 bridge reconnect, A2‑H4/H5 wizard download+installer hardening, A1‑H1 delete-key guard, A1‑H2 PostProcessing masked key, A1‑H3 `open_file` allowlist, A1‑H4 import rollback, A1‑H5/A2‑M7 whisper requeue + override readiness.
2. **Wave 2 — Perf & UX correctness:** A2‑M8 embed-lock/spawn_blocking + diarizer cache, A1 cancel→distinct status, A2‑M6 meeting-stop best-effort, A2‑M1 poisoned download, A2‑M21 per-request override, A1 server-side kind filter.
3. **Wave 3 — CLI/doctor/config correctness:** A2‑M3 `config set`, A2‑M4 status no-auto-spawn, A2‑M5/M15 doctor, A2‑M13/M14 config validate+expand, A2‑M9 pipe-busy deadline, A2‑M22 fix `ActionRow.test.ts`.
4. **Wave 4 — Hardening & data integrity:** A2‑M12 queue crash-dup, A2‑M20 retention transaction, A2‑M10 U16 audio, A2‑M11 import OOM, A2‑M16/M17 bounded error bodies, A2‑M18 profile hotkeys, A2‑M19 overlay capabilities, A1 webhook SSRF + queue-IPC tests.
5. **Wave 5 — Low / docs / DX:** the Low tier + doc drift + ESLint.
