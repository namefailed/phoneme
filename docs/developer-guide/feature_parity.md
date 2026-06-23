# Feature parity across surfaces

Phoneme exposes its capabilities through five surfaces. They are not meant to be
identical — only the GUI, CLI, and IPC daemon wire aim for full coverage; REST
and MCP are deliberately curated, thin bridges. This page is the canonical
cross-surface map: what each capability looks like on each surface, and where the
asymmetries are.

## The five surfaces

| Surface | What it is | Coverage intent |
|---|---|---|
| **GUI** | The Tauri desktop app. Calls the daemon over IPC through `src-tauri` Tauri commands; reads/writes `config.toml` directly for config. | Full — the reference surface. |
| **CLI** (`phoneme`) | `bin/phoneme/src/args.rs` is the authoritative command surface; dispatch lives in `bin/phoneme/src/commands/*`. Connects to the daemon over IPC, or runs in-process for local-file ops. | Full — headless parity with the GUI is a standing goal. |
| **IPC** | The daemon wire contract: the `Request` enum in `crates/phoneme-ipc/src/schema.rs`. One JSON object per line over the named pipe (default `\\.\pipe\phoneme-daemon`), each answered by one `Response`. Both the GUI and CLI are IPC clients. | Full — every daemon capability is an IPC variant (a handful of GUI/CLI things are local-file ops with no IPC verb). |
| **REST** (`phoneme-rest`) | An optional loopback-only (`127.0.0.1`) HTTP+SSE bridge. **Off by default** (`[rest_api].enabled = false`, default port `3737`). Each route maps one HTTP call to exactly one IPC `Request` and returns the daemon's JSON verbatim. See [rest_api.md](rest_api.md). | **Curated, not full parity.** Read + light-mutate. ~24 routes. |
| **MCP** (`phoneme-mcp`) | The agent surface: a Model Context Protocol server over the same daemon. A hand-picked set of agent-relevant tools, also a thin one-call-per-tool bridge. See [mcp_server.md](mcp_server.md). | **Curated, not full parity.** ~30 task-shaped tools. |

REST and MCP being thin is a design choice, not a backlog. Both are one-call-per-
endpoint bridges with no business logic — the daemon stays the single source of
truth. REST leans read-only; MCP exposes only what an agent would reasonably
drive. The "Known gaps" section below tags each missing cell as a real code-gap,
an intentional omission, or a doc-gap so you can tell which is which.

A cell reads `yes` (present, with the surface's identifier in parentheses),
`no` (the surface could carry it but doesn't), or `n/a` (it does not make sense
on that surface — e.g. shell completions on REST). Notes call out where a
present cell is narrower than the GUI/CLI (no overrides, hold-only, unscoped,
etc.).

---

## Parity matrix

### Recording

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Start recording | yes (`recordStart`) | yes (`record` / `record start`) | yes (`RecordStart`) | yes (`POST /api/record/start`, hold only) | yes (`start_recording`) | REST/MCP can't pass recipe/model/source/in-place overrides; REST is always hold mode. |
| Stop recording | yes (`recordStop`) | yes (`record stop`) | yes (`RecordStop`) | yes (`POST /api/record/stop`) | yes (`stop_recording`) | Full parity. |
| Toggle recording | yes (button/hotkey) | yes (`record toggle`) | yes (`RecordToggle`) | no | no | Atomic start-stop for hotkeys/UI; REST/MCP use explicit start+stop. |
| Cancel recording | yes (`recordCancel`) | yes (`record cancel`) | yes (`RecordCancel`) | no | no | Discard active recording without saving. |
| Pause recording | yes (`recordPause`) | yes (`record pause`) | yes (`RecordPause`) | no | no | Pause active recording/meeting. |
| Resume recording | yes (`recordResume`) | yes (`record resume`) | yes (`RecordResume`) | no | no | Resume paused recording/meeting. |
| Record status (re-sync) | yes (used on load) | n/a | yes (`RecordStatus`) | no | no | GUI re-syncs record buttons; CLI uses `daemon status`. |
| Set live-preview meeting source | yes (meeting-view toggle) | n/a | yes (`SetPreviewSource`) | no | no | Meeting-only live-preview toggle. |

### Meetings

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Start meeting | yes (`startMeeting`) | yes (`meeting start`) | yes (`StartMeeting`) | yes (`POST /api/meeting/start`) | yes (`start_meeting`) | Full parity. |
| Stop meeting | yes (`stopMeeting`) | yes (`meeting stop`) | yes (`StopMeeting`) | yes (`POST /api/meeting/stop`) | yes (`stop_meeting`) | Full parity. |
| Toggle meeting | yes (button/hotkey) | yes (`meeting toggle`) | yes (`MeetingToggle`) | no | no | Atomic hotkey-oriented variant. |
| List meeting tracks | yes (`listSession`) | yes (`meeting tracks`) | yes (`ListMeeting`) | no | yes (`list_meeting`) | No REST meeting-track route. |
| Meeting digest (generate/regen) | yes (`rerunMeetingDigest`) | yes (`meeting digest`) | yes (`RerunMeetingDigest`) | no | no | No REST/MCP meeting-digest synthesis. |
| Get meeting digest | yes (`getMeetingDigest`) | yes (`meeting digest --show`) | yes (`GetMeetingDigest`) | no | no | Read of stored meeting digest. |
| Rename meeting session | yes (`updateMeetingName`) | yes (`meeting rename`) | yes (`UpdateMeetingName`) | no | no | Set/clear meeting display name. |
| Delete meeting session | yes (`deleteSession`) | n/a (delete is per-recording) | yes (`DeleteSession`) | no | no | CLI deletes recordings one at a time; no whole-session verb. |

### Library — import / read / edit

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Import audio file | yes (`importRecording`, file picker) | yes (`import <file>`) | yes (`ImportRecording`, path only) | no | no | Core import on GUI/CLI/IPC; absent on REST/MCP. |
| Import from URL (yt-dlp) | no | yes (`import <url>`) | no | no | no | CLI-only: yt-dlp download then path import; IPC takes a local path only. |
| List recordings (filter/paginate) | yes (`listRecordings`) | yes (`list`) | yes (`ListRecordings`) | yes (`GET /api/recordings`) | yes (`list_recent`) | REST exposes limit/offset/kind; MCP `list_recent` is newest-first only. |
| Show one recording | yes (`getRecording`) | yes (`show`) | yes (`GetRecording`) | yes (`GET /api/recordings/{id}`) | yes (`get_transcript` / via search) | MCP `get_transcript` returns transcript text only. |
| Get transcript segments | yes (`getSegments`) | yes (`show --segments`) | yes (`GetSegments`) | yes (`GET .../segments`) | yes (`get_segments`) | REST/MCP always request the default variant (no cleaned). |
| Get per-word layer | yes (`getWords`) | n/a | yes (`GetWords`) | yes (`GET .../words`) | yes (`get_words`) | Word timings for views/captions; no CLI verb. |
| Get original/unedited transcript | yes (via compare/views) | yes (`show --original` / `--unedited`) | yes (`GetOriginalTranscript` / `GetCleanTranscript`) | no | no | Preserved-copy reads. |
| Edit/update transcript | yes (`updateTranscript`, editor) | yes (`edit`) | yes (`UpdateTranscript`) | no | no | Hand edit of the live transcript. |
| Find/replace in one recording | no | yes (`find-replace <id>`) | yes (`FindReplace`) | no | no | `bridge.rs` allowlists it but no GUI/Tauri call wires it up. |
| Find/replace library-wide | no | yes (`find-replace --library`) | yes (`FindReplaceLibrary`) | no | no | Same — CLI + IPC only. |
| Get/set notes | yes (`updateNotes`) | yes (`notes`) | yes (`UpdateNotes`) | no | no | Free-form notes. |
| Set/clear title | yes (`setRecordingTitle`) | yes (`edit --title` / `--clear-title`) | yes (`SetRecordingTitle`) | yes (`POST .../title`) | yes (`set_title`) | Full parity. |
| Favorite/unfavorite | yes (`setFavorite`) | yes (`edit --favorite`) | yes (`SetFavorite`) | yes (`POST .../favorite`) | yes (`set_favorite`) | Full parity. |
| Pin/unpin | yes (`setPinned`) | yes (`edit --pin`) | yes (`SetPinned`) | yes (`POST .../pinned`) | yes (`set_pinned`) | Full parity. |
| Delete recording | yes (`deleteRecording`) | yes (`delete`) | yes (`DeleteRecording`) | no | yes (`delete_recording`) | No REST delete route (REST is read+light-mutate). |

### Transcript versions

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| List transcript versions | yes (`listTranscriptVersions`) | n/a | yes (`ListTranscriptVersions`) | no | no | Version chain is a GUI compare/diff feature. |
| Get transcript version | yes (used by compare) | n/a | yes (`GetTranscriptVersion`) | no | no | GUI-side version fetch. |
| Revert to version | yes (`revertToVersion`) | no | yes (`RevertToVersion`) | no | no | GUI-only revert; CLI has no version commands. |

### Re-runs (re-transcribe / post-process)

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Retranscribe | yes (`retranscribeRecording`) | yes (`retranscribe` / `replay`) | yes (`RetranscribeRecording`) | no | yes (`retranscribe`) | No REST route; MCP supports a one-time model override only. |
| Re-run cleanup (post-process) | yes (`rerunCleanup`, full overrides) | yes (`cleanup`) | yes (`RerunCleanup`) | yes (`POST .../cleanup`, no overrides) | yes (`rerun_cleanup`, no overrides) | REST/MCP use the configured provider only; GUI/CLI/IPC expose per-run provider/model/prompt/API overrides. |
| Summarize (generate/regen) | yes (`rerunSummary`) | yes (`summarize`) | yes (`RerunSummary`) | yes (`POST .../summary`, no overrides) | yes (`summarize`, no overrides) | REST/MCP have no per-run overrides. |
| Re-fire hook | yes (`refireHook`) | yes (`refire-hook`) | yes (`RefireHook`) | no | no | Re-run post-transcription hook on a stored transcript. |
| Test hook command | no | yes (`hook test`) | yes (`HookTest`) | no | no | IPC variant exists and `bridge.rs` allowlists it, but no Tauri command/frontend call wires it into the GUI Hook Manager. |

### Recall (search / Q&A / embeddings)

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Semantic search | yes (`semanticSearch`) | yes (`search` / `list --semantic`) | yes (`SemanticSearch`) | yes (`GET /api/search`) | yes (`search_recordings`) | REST search is unscoped (no facet filter); GUI/CLI/IPC support facets. |
| More like this | yes (`moreLikeThis`) | yes (`search --like`) | yes (`MoreLikeThis`) | yes (`GET .../similar`) | yes (`more_like_this`) | Full parity. |
| Ask archive (RAG Q&A) | yes (`ask`) | yes (`ask`) | yes (`Ask`) | no | no | Citation-grounded Q&A; GUI/CLI/IPC only. |
| Re-embed library | yes (`reembedAll`) | yes (`reembed`) | yes (`ReembedAll`) | no | no | Clear + re-embed; GUI/CLI/IPC only. |

### Diarization & speakers

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Speaker rename/clear name | yes (`setSpeakerName`) | yes (`speaker rename` / `clear`) | yes (`SetSpeakerName`) | no | yes (`set_speaker_name`) | Returns the name-propagation result. |
| Reassign segment speaker | yes (`reassignSegmentSpeaker`) | yes (`speaker reassign`) | yes (`ReassignSegmentSpeaker`) | no | yes (`reassign_speaker_segment`) | In-recording correction. |
| Merge speakers | yes (`mergeSpeakers`) | yes (`speaker merge`) | yes (`MergeSpeakers`) | no | yes (`merge_speakers`) | Within one recording. |
| Split speaker | yes (`splitSpeaker`) | yes (`speaker split`) | yes (`SplitSpeaker`) | no | yes (`split_speaker`) | Split segments onto a fresh label. |
| Calibrate voiceprint threshold | no | yes (`speaker calibrate`) | no | no | no | CLI-only, in-process read-only EER analysis over the catalog; no IPC variant. |
| Recognize speakers (on demand) | yes (`recognizeSpeakers`) | n/a | yes (`RecognizeSpeakers`) | no | yes (`recognize_speakers`) | Named-speaker suggestions; no CLI verb. |
| Dismiss speaker suggestion | yes (`dismissSpeakerSuggestion`) | n/a | yes (`DismissSpeakerSuggestion`) | no | no | GUI-side suggestion dismissal. |
| List named voices | yes (`listNamedVoices`) | n/a | yes (`ListNamedVoices`) | no | yes (`list_named_voices`) | Named-voice library has no CLI surface. |
| Rename named voice | yes (`renameNamedVoice`) | n/a | yes (`RenameNamedVoice`) | no | yes (`rename_named_voice`) | No CLI named-voice management. |
| Merge named voices | yes (`mergeNamedVoices`) | n/a | yes (`MergeNamedVoices`) | no | yes (`merge_named_voices`) | No CLI surface. |
| Forget named voice | yes (`forgetNamedVoice`) | n/a | yes (`ForgetNamedVoice`) | no | yes (`forget_named_voice`) | Reversible soft-delete; no CLI surface. |
| Undo forget named voice | yes (`undoForgetNamedVoice`) | n/a | yes (`UndoForgetNamedVoice`) | no | no | GUI-only undo; not in MCP. |

### Tasks, entities, chapters

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Suggest tasks (on demand) | yes (`suggestTasks`) | yes (`suggest-tasks`) | yes (`SuggestTasks`) | no | no | GUI/CLI/IPC only. |
| List tasks (library) | yes (`listAllTasks`) | yes (`tasks`) | yes (`ListAllTasks`) | no | no | Cross-recording task list. |
| Mark task done/undone | yes (`setTaskDone`) | yes (`tasks done` / `undone`) | yes (`SetTaskDone`) | no | no | GUI/CLI/IPC only. |
| Add task | yes (`addTask`) | yes (`tasks add`) | yes (`AddTask`) | no | no | GUI/CLI/IPC only. |
| Edit task | yes (`updateTask`) | yes (`tasks edit`) | yes (`UpdateTask`) | no | no | GUI/CLI/IPC only. |
| Delete task | yes (`deleteTask`) | yes (`tasks delete`) | yes (`DeleteTask`) | no | no | GUI/CLI/IPC only. |
| Reorder tasks | yes (`reorderTasks`) | yes (`tasks reorder`) | yes (`ReorderTasks`) | no | no | GUI/CLI/IPC only. |
| Task counts | yes (`taskCounts`) | n/a | yes (`TaskCounts`) | no | no | Sidebar badge counts; GUI/IPC only. |
| Suggest entities (on demand) | yes (`suggestEntities`) | yes (`suggest-entities`) | yes (`SuggestEntities`) | no | no | GUI/CLI/IPC only. |
| List entities (library facet) | yes (`listAllEntities`) | yes (`entities`) | yes (`ListAllEntities`) | no | no | Cross-recording entity facet. |
| Get entities for recording | yes (`getEntities`) | n/a (covered by `entities`) | yes (`GetEntities`) | no | no | Per-recording entity read. |
| Add entity | yes (`addEntity`) | yes (`entities add`) | yes (`AddEntity`) | no | no | GUI/CLI/IPC only. |
| Edit entity | yes (`updateEntity`) | yes (`entities edit`) | yes (`UpdateEntity`) | no | no | GUI/CLI/IPC only. |
| Delete entity | yes (`deleteEntity`) | yes (`entities delete`) | yes (`DeleteEntity`) | no | no | GUI/CLI/IPC only. |
| Merge entities (library) | yes (`mergeEntities`) | yes (`entities merge`) | yes (`MergeEntities`) | no | no | GUI/CLI/IPC only. |
| Generate chapters | yes (`suggestChapters`) | yes (`chapters`) | yes (`SuggestChapters`) | no | no | REST has read-only `GET .../chapters` but no generate. |
| Get chapters | yes (`getChapters`) | yes (`chapters --show`) | yes (`GetChapters`) | yes (`GET .../chapters`) | no | Read of stored chapters. |

### Tags

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Suggest tags (on demand) | yes (`suggestTags`) | yes (`suggest-tags`) | yes (`SuggestTags`) | no | yes (`suggest_tags`) | No REST route. |
| Approve tag suggestion | yes (`approveTagSuggestion`) | yes (`tag suggestions --approve`) | yes (`ApproveTagSuggestion`) | no | yes (`approve_tag_suggestion`) | No REST route. |
| Dismiss tag suggestion | yes (`dismissTagSuggestion`) | yes (`tag suggestions --dismiss`) | yes (`DismissTagSuggestion`) | no | yes (`dismiss_tag_suggestion`) | No REST route. |
| Clear all tag suggestions | yes (`clearAllTagSuggestions`) | yes (`tag clear-suggestions`) | yes (`ClearAllTagSuggestions`) | no | no | Bulk clear; GUI/CLI/IPC only. |
| List tags | yes (`listTags` / `listAllTags`) | yes (`tag list` / `--all`) | yes (`ListTags` / `ListAllTags`) | yes (`GET /api/tags`, attached only) | yes (`list_tags` = `ListAllTags`) | REST lists attached tags only; MCP lists all tags. |
| Create tag | yes (`addTag`) | yes (`tag add`) | yes (`AddTag`) | no | no | No REST/MCP tag creation. |
| Update/recolor tag | yes (`updateTag`) | yes (`tag update`) | yes (`UpdateTag`) | no | no | GUI/CLI/IPC only. |
| Delete tag | yes (`deleteTag`) | yes (`tag delete`) | yes (`DeleteTag`) | no | yes (`delete_tag`) | No REST route. |
| Attach tag to recording | yes (`attachTag`) | yes (`tag attach`) | yes (`AttachTag`) | yes (`POST .../tags`) | no | MCP can suggest/approve but not raw-attach an existing tag. |
| Detach tag from recording | yes (`detachTag`) | yes (`tag detach`) | yes (`DetachTag`) | yes (`DELETE .../tags/{tag_id}`) | no | No MCP detach. |
| Tags for recording | yes (`tagsFor`) | yes (`tag for`) | yes (`TagsFor`) | yes (`GET .../tags`) | n/a | MCP gets tags via the recording object. |
| Tag usage counts | yes (`tagUsageCounts`) | yes (`tag usage`) | yes (`TagUsageCounts`) | no | no | GUI/CLI/IPC only. |
| Merge tags | yes (`mergeTags`) | yes (`tag merge`) | yes (`MergeTags`) | no | no | GUI/CLI/IPC only. |

### Period digests

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Period digest (generate/regen) | yes (`rerunPeriodDigest`) | yes (`digest`) | yes (`RerunPeriodDigest`) | no | no | Date-window rollup; GUI/CLI/IPC only. |
| Get/list period digest | yes (`getPeriodDigest` / `listPeriodDigests`) | yes (`digest --show`) | yes (`GetPeriodDigest` / `ListPeriodDigests`) | no | no | Stored digest reads. |

### Export / backup

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Export clip (audio range) | yes (`exportClip`, waveform modal) | yes (`clip`) | yes (`ExportClip`) | no | no | Audio slice export. |
| Export captions (srt/vtt) | yes (`exportCaptions`) | yes (`export --captions`) | n/a (built from `GetSegments`/`GetWords`) | no | no (`get_words`/`get_segments` only) | Caption files assembled client-side; no single IPC verb. |
| Export library zip (backup) | yes (`exportLibraryZip`) | yes (`export <out>`) | n/a (composed of catalog reads) | no | no | Backup zip assembled from multiple reads; no dedicated IPC verb. |
| Import backup zip (restore) | no | yes (`import-backup`) | n/a (local file ops) | no | no | CLI-only restore; local operation against catalog/audio dir. |

### Dictation history

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Dictation history list | yes (`listDictationHistory`) | yes (`dictation history`) | yes (`ListDictationHistory`) | no | no | GUI/CLI/IPC only. |
| Regrab dictation | yes (`regrabDictation`) | yes (`dictation regrab`) | yes (`RegrabDictation`) | no | no | GUI/CLI/IPC only. |
| Forget/clear dictation history | yes (`delete` / `clearDictationHistory`) | yes (`dictation forget` / `clear`) | yes (`DeleteDictationHistory` / `ClearDictationHistory`) | no | no | GUI/CLI/IPC only. |

### Queue

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Queue list | yes (`listQueue`) | yes (`queue list`) | yes (`ListQueue`) | yes (`GET /api/queue`) | no | No MCP queue tools. |
| Queue counts | yes (`getQueueCounts`) | yes (`queue counts`) | yes (`QueueCounts`) | no | no | GUI/CLI/IPC only. |
| Pause/resume queue | yes (`setQueuePaused`) | yes (`queue pause` / `resume`) | yes (`SetQueuePaused`) | no | no | GUI/CLI/IPC only. |
| Queue paused status | yes (`queuePaused`) | yes (`queue status`) | yes (`QueuePaused`) | no | no | GUI/CLI/IPC only. |
| Reorder queue | yes (`reorderQueue`) | yes (`queue reorder`) | yes (`ReorderQueue`) | no | no | GUI/CLI/IPC only. |
| Cancel queued item | yes (`cancelQueued`) | yes (`queue cancel`) | yes (`CancelQueued`) | no | no | GUI/CLI/IPC only. |
| Cancel processing item | yes (`cancelProcessing`) | yes (`queue cancel-processing`) | yes (`CancelProcessing`) | no | no | GUI/CLI/IPC only. |
| Cancel all queued | yes (`cancelAllQueued`) | yes (`queue cancel-all`) | yes (`CancelAllQueued`) | no | no | GUI/CLI/IPC only. |
| Skip current LLM stage | yes (`skipCurrentStage`) | yes (`queue skip`) | yes (`SkipCurrentStage`) | no | no | GUI/CLI/IPC only. |
| Clear/dismiss failed | yes (`clearFailed` / `dismissFailed`) | yes (`queue clear-failed` / `dismiss-failed`) | yes (`ClearFailed` / `DismissFailed`) | no | no | GUI/CLI/IPC only. |

### Saved searches & AI activity

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Saved searches (list/upsert/delete/run) | yes (`listSavedSearches` etc.) | yes (`list --saved`) | yes (`ListSavedSearches` / `UpsertSavedSearch` / `DeleteSavedSearch` / `RunSavedSearch`) | no | no | CLI can run/list saved searches; create/delete is GUI/IPC. |
| AI activity history | yes (`listAiActivity`, brain popout) | n/a | yes (`ListAiActivity`) | no | no | GUI-only brain popout feature. |
| Kind counts (sidebar badges) | yes (`kindCounts`) | n/a | yes (`KindCounts`) | no | no | GUI sidebar counts; GUI/IPC only. |

### Config & daemon

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Run doctor (health checks) | yes (`runDoctor`) | yes (`doctor`) | yes (`RunDoctor`) | no | no | CLI runs shared checks in-process + uses the daemon for reachability. |
| Restart whisper server (Doctor fix) | yes (Doctor Fix → `restart_whisper`) | yes (`doctor --fix`) | yes (`RestartWhisper`) | no | no | GUI/CLI/IPC only. |
| Rebuild catalog (destructive) | no | yes (`doctor --rebuild-catalog`) | yes (`RebuildCatalog`) | no | no | Destructive repair; CLI/IPC only (no GUI button by design). |
| Reimport from disk (non-destructive) | no | yes (`doctor --reimport`) | yes (`ReimportFromDisk`) | no | no | CLI/IPC only; no GUI button. |
| Export diagnostics bundle | yes (`export_diagnostics`) | n/a | yes (`ExportDiagnostics`) | no | no | GUI System/Diagnostics button + IPC; no CLI verb yet. |
| Daemon status | yes (`daemonStatus`) | yes (`daemon status`) | yes (`DaemonStatus`) | yes (`GET /api/status`) | n/a | REST `/api/health` also probes it. |
| Daemon start/stop | yes (tray + auto-spawn) | yes (`daemon start` / `stop`) | yes (`Shutdown` for stop) | no | no | Start is a process spawn (CLI/tray); IPC only stops. |
| Reload config | yes (after Settings save) | yes (`config reload`) | yes (`ReloadConfig`) | no | no | GUI reloads after writing config; CLI/IPC explicit. |
| View config | yes (Settings UI) | yes (`config` / `--show-secrets`) | n/a (Tauri `config_io`) | no | no | GUI reads/writes `config.toml` via Tauri commands, not daemon IPC. |
| Set config value | yes (Settings UI, structured) | yes (`config set <key> <value>`) | n/a | no | no | GUI edits structured fields; CLI sets arbitrary dotted keys. Same intent, different mechanism. |
| Config path | n/a | yes (`config path`) | n/a | no | no | CLI convenience; the GUI knows its own path. |
| Profiles (list/save/use) | yes (`listProfiles` / `saveProfile` / `switchProfile` / rename / delete) | yes (`profile list` / `save` / `use`) | n/a (Tauri `config_io`) | no | no | GUI has richer profile management (rename/delete) than the 3 CLI subcommands. |

### Events & protocol

| Capability | GUI | CLI | IPC | REST | MCP | Notes |
|---|---|---|---|---|---|---|
| Watch event stream | yes (native event subscription) | yes (`watch`) | yes (`SubscribeEvents`) | yes (`GET /api/events` SSE) | no | MCP is request/response only; the other four surface the stream. |
| Shell completions | n/a | yes (`completions <shell>`) | n/a | n/a | n/a | Inherently CLI-only. |
| Protocol handshake | yes (on connect) | yes (on connect) | yes (`Handshake`) | n/a | n/a | Wire-version negotiation, internal to IPC clients. |
| Version info | yes (About/Settings) | yes (`version` / `--version`) | yes (via `DaemonStatus`/`Handshake`) | yes (via `/api/status`) | n/a | Reported across surfaces. |

---

## Known gaps

Each gap is one of three kinds:

- **code-gap** — a genuine asymmetry worth closing; the plumbing usually already
  exists on the daemon side.
- **intentional** — a deliberate scope decision (REST/MCP thinness, destructive
  ops kept off the GUI, CLI-only network tools, etc.). Not a bug.
- **doc-gap** — present in code but missing/wrong in docs. (None outstanding here;
  the cross-surface doc fixes are tracked separately in the docs audit.)

### code-gap

- **Test hook command** — missing on GUI/REST/MCP; present on CLI (`hook test`)
  and IPC (`HookTest`). `src-tauri/src/bridge.rs` explicitly allowlists `HookTest`
  for forwarding, but no Tauri command or frontend call invokes it, and the Hook
  Manager UI has no "test this command" button. The plumbing is half-built. A
  Test button in the Hook Manager closes it. (Task #257.)
- **Find/replace (single + library)** — missing on GUI/REST/MCP; present on CLI
  (`find-replace`) and IPC (`FindReplace` / `FindReplaceLibrary`). `bridge.rs`
  allowlists both for the WebView but nothing calls them. The daemon already does
  the safe preserve-and-re-flow per recording. Low-effort, high-value GUI add.
  (Task #256.)
- **Named-voice library (list/rename/merge/forget)** — missing on CLI; present on
  GUI, IPC, and (mostly) MCP. The whole cross-recording named-voice library has
  zero CLI surface — no `phoneme voice ...` command — so a headless user who
  enrolls voices via dictation/recognition can't manage them from the CLI. A
  `phoneme voice` command group restores parity. (Task #258.)
- **Export diagnostics bundle** — missing on CLI; present on GUI and IPC
  (`ExportDiagnostics`). The daemon fully supports it but a headless/server user
  can't produce a support bundle from the command line. A `phoneme doctor
  --diagnostics` (or `phoneme diagnostics`) verb closes it. (Task #259.)

### intentional

- **Calibrate voiceprint threshold** — CLI-only by design. `run_calibrate`
  (`bin/phoneme/src/commands/speaker.rs`) opens the catalog directly as a
  concurrent WAL reader and runs the pure EER metric in-process; it is read-only
  and only *suggests* a `voiceprint_match_threshold` (never writes config). An
  advanced tuning step; a GUI "suggest threshold" button would be a nice-to-have,
  not a parity bug.
- **Rebuild catalog / Reimport from disk** — kept out of the one-click GUI Doctor.
  `RebuildCatalog` is destructive (drops transcripts/tags/edits) and
  `ReimportFromDisk` is its safe counterpart; both are deliberately CLI/IPC-only
  expert repair commands to avoid accidental data loss. The GUI Doctor exposes the
  non-destructive `restart_whisper` fix only.
- **Undo forget named voice** — `forget_named_voice` is in MCP but its undo is
  not. The undo is framed as a GUI safety affordance: the destructive op is
  reversible, and the reversal lives where the user did it.
- **Import from URL (yt-dlp)** — CLI-only. IPC `ImportRecording` takes a local
  absolute path only; the yt-dlp download wrapper shells out to yt-dlp + a JS
  runtime and lives entirely in the CLI. Keeping a network-download tool out of
  the always-on daemon/GUI is a local-first/security choice (the GUI uses a native
  file picker → path).
- **Bulk-knowledge surface on REST/MCP** — tasks, entities, period/meeting
  digests, ask, re-embed, queue control, tag CRUD, find/replace, etc. are out of
  scope for REST (a thin read+light-mutate loopback bridge) and MCP (a curated
  agent surface). One HTTP call = one Request; agent-relevant tools only. Revisit
  only if a specific external integration needs one.
- **Generate chapters/entities/tasks on REST** — REST exposes `GET
  .../chapters` (read) but no generate route, and no entities/tasks routes —
  consistent with its read-leaning scope. Chapters is the one place REST has the
  read half but not the write half.
- **Attach/detach existing tag on MCP** — MCP routes tagging through
  `suggest_tags` / `approve_tag_suggestion` (the LLM-assisted flow) and omits raw
  `AttachTag`/`DetachTag`, keeping the agent surface task-shaped rather than a thin
  DB editor. REST does expose raw attach/detach.

---

See also: [IPC Integration Guide](ipc_integration.md) (the wire contract every
bridge forwards to), [Local REST API](rest_api.md), [MCP server](mcp_server.md),
and the [CLI reference](cli_reference.md).
