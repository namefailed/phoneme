# 🗺️ Phoneme Roadmap

The single source of truth for **where Phoneme is going**. Shipped history lives in
[`CHANGELOG.md`](CHANGELOG.md); speculative / unvetted ideas live in the
[Idea Parking Lot](docs/IDEAS.md).

---

## Who we build for

Phoneme isn't one app — it's four overlapping workflows. Every roadmap item should
move at least one of these personas closer to "done":

| Persona | Core job | "Done" feels like |
|---|---|---|
| **Dictator** | Hotkey → speak → text lands in Slack/Word/IDE | Zero friction, never think about modes |
| **Meeting archivist** | Capture a call + your reactions → readable timeline → summary | One chronological story, not two blobs |
| **Recall librarian** | Find "that thing I said about Rust errors last week" | Meaning-search + organization that scales past 5k rows |
| **Automator** | Every recording fires Obsidian/Discord/scripts | Reliable, debuggable, configurable without editing TOML |

## Guiding principles

1. **"Would a real user hit this friction?"** — features that duplicate existing
   functionality or serve <~10% of users get cut or parked, not built.
2. **Missing buttons, not missing features.** The backend + CLI are consistently
   *ahead of the GUI*. A large share of high-value work is **wiring + polish**, not
   greenfield engineering (webhook config, semantic scores, failed-queue, delete
   modes, catalog rebuild all exist in the backend with no UI). Close that gap first.
3. **Differentiation lives in Recall and Meetings**, not Dictation. Dictation is
   table-stakes (keep it frictionless); the moat is "search/ask your own voice
   archive" and "dual-track diarized meetings."
4. **Respect dependencies.** Some "separate" features share one substrate — build
   the substrate once (see the ⚠️ callouts below).

---

## ✅ Recently shipped (this cycle)

Landed most recently — verified against current code:

- [x] **Auto-tagging** — a `Tagging` pipeline stage where an LLM suggests tags for
  approval (✓/✕ per tag, ✓ All / ✕ All), with its own `[auto_tag]` provider/model/
  prompt (inheriting `[llm_post_process]` where blank), a max-tags cap, and an
  `auto_accept_existing` toggle that silently applies suggestions matching tags the
  library already has while queueing genuinely new ones for review.
- [x] **Doctor self-healing** — `RestartWhisper` sweeps stray `whisper-server`
  processes and bounces both supervisors (backoff reset), surfaced as a header
  health pill + a failure banner, `phoneme doctor --fix` on the CLI, and the
  Doctor view reachable from the main UI (`g D`).
- [x] **True split panes** — `\` opens a second full recording pane (independent
  editor/actions/dirty state, draggable ratio-persisted splitter, per-pane Esc);
  replaces the old side-by-side modal.
- [x] **Meeting live preview** — live captions during meetings with a `toggle`
  mode (🎤/🔊 source switch on the overlay) or an optional `both` mode streaming
  the two tracks at once.
- [x] **Keyboard & layout overhaul** — vim cursor persists across reload and pane
  switches (dimmed when unfocused), `zz` centers the list, `g d` jumps to the
  detail pane, `g /` to search, contextual `f` zen (recording open → focus mode,
  else full-window list) with chrome snapshot/restore, a real animation system
  (`interface.animation_speed`: off/fast/normal/slow, honors reduced-motion),
  Ctrl+/ top-bar toggle, list zoom, sidebar + queue 2D nav, and a help sheet that
  lists every binding.
- [x] **Named speakers** — rename `Speaker N` once in the detail or merged view;
  persisted (`speaker_names` migration), rewrites the transcript text, and stays
  re-renamable afterwards.
- [x] **Saved searches & favorites** — saved searches capture the *full* filter
  state and get a manager (Settings → Managers, `g S`); recordings can be starred
  (list column + a Library "Favorites" filter, persisted in the catalog).
- [x] **Queue panel polish** — inverted order with the active item pinned,
  skip-current-stage (`SkipCurrentStage` IPC), failed badge + clear, and the list
  pinned to the bottom on load/additions.
- [x] **Transcript version diff** — side-by-side compare of original Whisper vs
  cleaned vs current edit (`TranscriptDiff.ts`).
- [x] **Summary errors carry the real reason** — a failed summary names the
  provider/endpoint actually used (including a per-step override), instead of a
  generic "check the AI provider".
- [x] **Docs overhaul** — the developer guide is now a code wiki (internals,
  onboarding, how-to-extend, frontend + backend guides) and every user-facing
  feature and CLI command is documented; CI green throughout, with the release
  workflow gated on the same checks.

## ✅ Recently shipped (previous cycle)

- [x] **Chunked hybrid semantic search** — sentence-aware chunking (`chunk.rs`) +
  per-chunk embeddings (`embedding_chunks`) fused with FTS5 via RRF
  (`fusion.rs`, `catalog.rs::hybrid_search`) and a calibrated 0–100% relevance.
  Fixes paraphrase recall on longer notes. *(closes the v1.10 chunking item early)*
- [x] **Embedding model as a user choice** — `[semantic_search]` now carries
  `max_tokens`, `pooling` (mean/cls), `token_type_ids`, and `query_prefix` /
  `passage_prefix`, so E5/BGE-class models work, not just all-MiniLM; a dedicated
  **Semantic Search** settings section plus a **Re-embed library** action
  (`ReembedAll`) re-index everything after a model change.
- [x] **Merged meeting view** — selecting a meeting's group header renders all
  tracks as one read-only, source-sectioned, speaker-aware document with Copy /
  Export (`MergedConversationDetail.ts`, `mergeMeeting.ts`). Coarse, not yet
  chronological — see the v1.9 Meetings item.
- [x] **System-wide live-preview overlay** — an opt-in, always-on-top, frameless
  desktop window that floats the live caption over any app (`src-tauri/src/overlay.rs`,
  `frontend/overlay.*`), gated on `interface.preview_overlay`.
- [x] **Masked config at the WebView boundary (S-H2)** — API keys are masked before
  reaching the renderer and restored on save (`src-tauri/src/commands.rs`).
- [x] **IPC connection resilience** — an unknown/unparseable request now returns an
  error `Response` instead of tearing down the pipe (`ServerRequest::Unknown`,
  `phoneme-ipc`).
- [x] **Queue failed-count + clear** and **Import audio** in Settings → Storage.

## ✅ Recently shipped (post-v1.8 baseline)

Landed since the last roadmap reorg — these close several promise-vs-reality gaps:

- [x] **Independent provider system** — transcription, live preview, cleanup, and
  summary each pick their **own** provider + model. Shared catalogs ship one-click
  presets for STT (local whisper.cpp, OpenAI, Groq, Deepgram, AssemblyAI,
  ElevenLabs, custom OpenAI-compatible) and LLM (Ollama, LM Studio, Jan, llama.cpp,
  OpenAI, Anthropic, Groq, Gemini, Mistral, DeepSeek, OpenRouter, Together, xAI,
  Cerebras, Fireworks, DeepInfra, Perplexity, Nebius, Hyperbolic). LLM model fields
  fetch live `/models`; STT fields use curated lists + an "Other" fallback.
- [x] **Auto AI Summary** — per-recording LLM summary, on demand (**View summary**)
  or automatically as the final pipeline step (`summary.auto`), with an independent
  `[summary]` provider/model/prompt and a `RerunSummary` IPC. Stored in
  `recordings.summary` / `summary_model`.
- [x] **Three transcript layers** — raw machine output (`original_transcript`),
  cleaned-but-unedited pipeline output (`clean_transcript`), and the current edited
  transcript (`transcript`), each viewable + restorable in the detail view.
- [x] **Reworked First Run Wizard** — multi-step (Welcome → Mode → Setup → Connect
  AI → Mic → Live Preview → Auto Summary → Destination → Hotkeys → Review → Done),
  with a unified "Connect AI" key-entry step and local-dependency installs.
- [x] **Settings overhaul** — search, six grouped tabs, Live Preview config, the
  Post-Processing (cleanup + summary) section, and a per-recording **Re-run** menu
  with one-time overrides.

---

## 🔧 In flight — v1.8.x (correctness & performance)

Landing now as focused PRs (each tested, `clippy -D warnings` clean):

- [x] **Live preview no longer O(n²)** — the streaming preview re-transcribed the
  *entire* growing buffer every tick; now bounded to a rolling 15 s window.
- [x] **Diarization speaker mapping fixed** — pyannote `"SPEAKER_00"` labels were
  `parse::<u8>()`'d and collapsed everyone to one speaker; now mapped to stable
  indices with gap handling, and run off the async runtime.
- [x] **Semantic search hardened** — dimension check, relevance floor (drop noise),
  and meeting-track dedupe.
- [x] **Embedding input truncated** to the model's 256-token limit (long transcripts
  silently failed to embed → became unsearchable).
- [x] **In-place dictation surfaces errors** instead of panicking / silently no-op'ing.
- [x] **Meeting tracks stay synced across silence** — loopback (system audio) is
  captured continuously by filling real silence for wall-clock gaps, so pausing
  a video mid-meeting no longer collapses the gap and desyncs the tracks; the
  fill is pause-aware (a meeting pause freezes both tracks, no back-filled silence).
- [x] **Transcriptions no longer time out under live preview** — a whisper-server
  semaphore makes the preview yield to the real transcription, fixing the
  `"Whisper timed out after 60s"` failures on long recordings / meetings.

> These are the baseline for v1.9 — several items below build directly on them.

---

## 🔒 Security hardening (from June 2026 audit)

*Posture today: solid app-layer hygiene (parameterized SQL, FTS5 sanitization,
shlex hooks with timeouts, redacted Debug, no `unsafe`), but a weak platform
trust boundary. Fit for a single trusted Windows user; not hardened against
same-user malware or a malicious IPC client. Ordered by priority.*

**Near-term (the trust boundary)**
- [x] **Named-pipe access control** — owner-only SDDL (`D:P(A;;GA;;;OW)(A;;GA;;;SY)`)
  on every pipe instance removes the default cross-session `GENERIC_READ` (which
  exposed the transcript event stream). Same-user isolation (an auth token)
  remains open. *(audit S-C1/S-H8)*
- [x] **Hook execution allowlist over IPC** — `RefireHook` now only runs a
  command already in the configured hook allowlist (arbitrary IPC commands are
  rejected). `HookTest` intentionally still runs a typed command — it's the Hook
  Manager's test affordance, gated by the owner-only pipe (S-C1). *(S-C2)*
- [x] **IPC frame size cap** (`codec.rs`) — NDJSON frames are bounded at 8 MiB;
  an unterminated over-cap frame errors instead of growing the buffer. *(S-H6)*
- [x] **Path guards** — `reveal_file` restricts the target to `audio_dir`;
  audio deletion rejects `..` and paths outside `audio_dir`. *(S-H3/S-H5)*
- [x] **`escapeHtml` the RecordingDetail error path** (`RecordingDetail.ts:59`). *(S-medium)*

**Secrets & transport**
- [x] **Stop sending full API keys to the WebView** — `read_config` now masks every
  non-empty key (`__phoneme_secret_kept__`) before it crosses to the renderer, and
  `write_config` restores any unchanged key from disk, so secrets never reach the
  WebView (`src-tauri/src/commands.rs` `mask_config_secrets`/`unmask_config_secrets`).
  Encrypting them at rest (below) is the remaining half. *(S-H2)*
- [x] **Encrypt secrets at rest** (Windows DPAPI) — API keys are encrypted per-user
  with `CryptProtectData` (a `dpapi:v1:` prefix) on write and transparently decrypted
  on load; legacy plaintext keys migrate on the next save, and an undecryptable blob
  reads as unset rather than leaking. Composes with the S-H2 masking (the mask sees
  the encrypted value, still replaces it with the sentinel). *(S-H2 — both halves now
  done. `phoneme-core::secret_crypto`, `config.rs` serde.)*
- [x] **Webhook SSRF guard** — the webhook client classifies every target before
  POSTing: loopback always allowed (local-first), other private ranges blocked
  unless `[webhook] allow_private_network = true`, public targets HTTPS-only
  unless `[webhook] allow_http = true`; hostnames resolve-and-classify, redirects
  never followed (`phoneme-core::webhook`). HMAC signing still later. *(S-H1)*
- [x] **Baseline CSP + narrowed asset/fs scopes** — *shipped*: real prod CSP + devCsp, asset scope narrowed to the audio + app-data dirs, unused window capabilities dropped. Was: (`tauri.conf.json` is `csp:null`,
  `$HOME/**`). *(S-H4 — also tracked under Long Term → Security)*
- [x] **Model-download checksums** — *shipped*: every wizard artifact pinned (HF lfs.oid / release digest), zip verified before extraction, unpinned allowed-host URLs fail closed. *(S-H7)*

**Hygiene**
- [x] **`cargo audit` + `pnpm audit` in CI** (non-blocking advisory job; gate core crates later). *(also in tech-debt backlog)*
- [x] Hook `HookTest` stderr may contain secrets — now redacted before returning:
  `phoneme-core::hook::redact_secrets` masks credential-shaped tokens (and caps
  length) on both the success and failure paths of the daemon's `HookTest`.
- [x] A short **threat-model doc** capturing these boundaries. → [docs/developer-guide/threat_model.md](docs/developer-guide/threat_model.md)

---

## 📋 v1.9 — Completeness & Recall

**Theme: close the promise-vs-reality gaps and finish the attic.** Most of this is
wiring features the backend already supports. Meetings-first: the coarse merged
view shipped, but the *chronological* interleaved timeline — the thing the
persona actually wants — still needs the alignment + timestamp substrate below.

### ⚠️ Prerequisites / shared infrastructure (do these first)

- [ ] **Meeting-track alignment correctness** — the merged timeline is *not* pure UI
  wiring; it depends on the two tracks being truly time-aligned. The current
  `meeting_align.rs` heuristic is fragile (it can collapse internal silence when the
  system-audio loopback drops gaps). An interleaved timeline built on mis-aligned
  tracks is *worse* than two stacked panes. **Solidify alignment before the merged
  view.**
- [x] **Word-level timestamps** — shared substrate for transcript↔waveform sync,
  confidence highlighting, *and* tighter diarization boundaries. *Shipped:*
  providers capture per-word timing + confidence into `transcript_words`
  (`GetWords` IPC / `get_words` Tauri command); the detail pane's **🔤 Synced**
  peek renders the machine transcript as clickable, time-coded words (click →
  seek, playhead-follow highlight). Confidence highlighting and word-level
  diarization build on this substrate.

### 🏚️ Finish the attic (backend exists, GUI doesn't)

### 🔁 GUI ⇄ CLI parity gaps (audited 2026-06-12 against the live Request enum)

The CLI-parity pass predates several newer features. CLI is missing:

- [x] **Title set/clear** — `SetRecordingTitle` has no CLI verb (titles are
  brand new); `phoneme edit --title` / `--clear-title` or similar.
- [x] **Favorites** — `SetFavorite` unreachable from the CLI (star/unstar).
- [x] **Speaker rename** — `SetSpeakerName` unreachable (named speakers are
  GUI-only).
- [x] **Tag-suggestion review** — approve/dismiss per suggestion
  (`ApproveTagSuggestion` / `DismissTagSuggestion`); only clear-all exists.
- [x] **Record pause/resume** — `RecordPause` / `RecordResume` have no CLI
  verbs (start/stop/toggle/cancel do).
- [x] **Queue skip** — `phoneme queue skip` sends `SkipCurrentStage` (observe-only: never auto-spawns a daemon just to skip nothing).
- [x] **Re-run tag suggestions** — `SuggestTags` missing beside the existing
  rerun cleanup/summary verbs.

GUI is missing:

- [x] **Caption export button** — a 💬 Captions SRT/VTT split-menu on the detail-pane action row (no-segments → retranscribe hint).
- [x] **Webhook safety knobs** — Allow-private-network / Allow-insecure-HTTP toggles in SectionHook with warning copy.
- [x] **Whole-library export** — Settings → Storage "Back up to .zip…" produces the CLI-equivalent catalog+audio archive (the old text Export is now labelled text-only).

House rule going forward: a new Request lands with BOTH surfaces (or an
explicit roadmap line here saying why not).

- [x] **Webhook URL field in Settings** — the Hooks section now exposes the
  `hook.webhook_url` field (with empty-value guarding); the pipeline POSTs to it. (`SectionHook.ts`)
- [x] **Failed-queue visibility + clear** — the queue panel now surfaces the
  `failed/` count as a badge and lets the user dismiss it (`QueuePanel.ts`
  `clearFailed` → `ClearFailed`/`getQueueCounts` IPC; the `queue_depth_changed`
  event carries the failed count). *Per-file error detail + one-click **retry** is
  still pending* — today clear only removes the failed marker; the recording and
  its transcript are untouched.
- [ ] **Doctor: rebuild catalog** — `phoneme doctor --rebuild-catalog` is CLI-only.
- [x] **Delete modes (keep-audio / transcript-only)** — the delete confirmation
  (single, bulk, `Delete` key, `dd`) now offers **Delete everything** (default) or
  **Keep the audio file** (`ConfirmDelete.ts` `confirmRecordingDelete` →
  `delete_recording { keep_audio }`); one chosen mode applies to the whole bulk
  selection, the Undo grace period covers both, and "Don't ask again" replays the
  mode chosen when it was set.
- [x] **Bulk tag from multi-select** — the floating bulk bar now has Tag plus the full shared Re-run form, Export, and Delete.
- [x] **Semantic search settings + re-index** — a dedicated **Semantic Search**
  settings section (`SectionSemantic.ts`) exposes the toggle, model directory, and
  the embedding-model knobs (max tokens, pooling, `token_type_ids`, query/passage
  prefixes), plus a **Re-embed all recordings** action (`ReembedAll` IPC) that
  clears every vector and re-indexes the library in the background. It lives under
  the **System** tab (and is also surfaced via Settings **search**).
- [ ] **IPC reconnect after Doctor "Fix"** — today users must close/reopen the window after a daemon restart.
- [ ] **In-app hook log tail** — hook debugging means opening `%LOCALAPPDATA%\phoneme\logs\hook.log` by hand.
- [x] **Import file picker** — wired as an **Import audio** button in Settings →
  Storage (`SectionStorage.ts` → `pickAndImportAudio`), alongside drag-drop.
- [x] **FLAC import** — symphonia `flac` feature enabled; wav/mp3/m4a/flac all accepted.
- [x] **Recording mode on the main button** — the Record split-button dropdown
  gained an **"A voice note stops"** group: **When I click Stop** (wire `hold`),
  **When I go quiet** (`oneshot`), or **After N seconds** (`duration:N`, inline
  seconds field). Persisted per device (`recordStopMode.ts`), shown in the button
  tooltip; with no explicit choice the old `auto_stop_on_silence` default still
  applies. Push-to-talk hold stays hotkey-only — a click can't be held.

### 🎙️ Meetings

- [x] **Merged meeting view (coarse — chronological interleave still pending)** — a
  source-sectioned, speaker-aware merge shipped
  (`MergedConversationDetail.ts` / `mergeMeeting.ts`): selecting a meeting's
  group header renders every track as one read-only document, labelled 🎤 Microphone
  / 🔊 System audio with the pipeline's `[Speaker N]` turns surfaced, plus Copy /
  Export. It does **not** yet interleave the tracks *chronologically* — per-line
  timestamps aren't persisted, so a true "You / Meeting" timeline still depends on
  the alignment + word-timestamp prerequisites above.
- [ ] **Diarization quality** *(prereq for named speakers — don't build naming UX on wrong labels)*. Each item below was verified against `diarization.rs` / `transcription.rs` and the `speakrs 0.4.2` source; verdicts noted inline.
  - [x] **Fix the `to_segments` frame scaling, then coalesce the turns** ✓ *(shipped)*. #23 first dropped the old manual `result.discrete_diarization.to_segments(1.0, 1.0)`, whose `(1.0, 1.0)` `frame_step`/`frame_duration` (vs speakrs' real `FRAME_STEP_SECONDS = 0.016875` / `FRAME_DURATION_SECONDS = 0.0619375`) inflated every turn ~59× and scrambled `assign_speakers`. But `result.segments` is **not** usable raw: speakrs builds it via `to_segments(…) + merge_segments(merge_gap)` with `PipelineConfig::default().merge_gap == 0.0` — a no-op merge — and emits **per-speaker** spans sorted only by start, so one speaker's speech fragments on every micro-pause and different speakers' spans interleave → flickering `[Speaker N]` labels. Now `clean_speaker_spans` sorts, drops zero-length spans, and merges adjacent same-speaker turns under 0.25 s, and `speaker_for_segment` attributes each transcript line by **max temporal overlap** (the old midpoint-first-covering-match could collapse an overlapped line onto whichever turn merely started first). *(diarization.rs; 7 new unit tests, one verified to fail under the old logic.)*
  - [x] **Cache the pipeline in `AppState`** — *shipped* (lazily, config-keyed, inside `Transcriber`; loads once on first diarized run instead of at startup). Was: `run_local_diarization` calls `OwnedDiarizationPipeline::from_pretrained(ExecutionMode::Cpu)` on *every* transcription (`diarization.rs:157`), reloading the ~500 MB seg+emb ONNX models each time; `AppState` (`app_state.rs`) holds no diarizer. Hold one long-lived pipeline fed via speakrs' background queue — `OwnedDiarizationPipeline::into_queued()` returns a `(QueueSender, QueueReceiver)` (`pipeline.rs:179`) — so model load happens once at startup. *(transcription.rs:352 `diarize_transcript` → diarization.rs:154)*
  - [ ] **Track-aware Meeting Mode** *(confirmed)*. `diarize_transcript` runs speakrs identically for every recording; there is no branch on `MeetingTrack::Mic` vs `System` (transcription.rs only sees a path + segments, never the track; the track lives in the catalog row, recorder.rs:854–857). For meetings, label the mic track **"You"** without running speakrs at all, and only diarize the system/loopback track — halves diarizer work and avoids spurious multi-speaker labels on a single-mic track. *(transcription.rs:352; recorder.rs `MeetingTrack::Mic/System`)*
  - [ ] **Word-level alignment instead of 1 s segments** *(confirmed)*. Today's path is whisper **segments** × diarization turns: the local provider requests `timestamp_granularities[]=segment` (transcription.rs:285) and `assign_speakers` attributes each whole segment by its midpoint (diarization.rs:90). Request `timestamp_granularities[]=word` from whisper-server and assign each *word* to a speaker via the per-frame activation matrix — `DiscreteDiarization` derefs to a public `Array2<f32>` of frame activations (`pipeline/types/data.rs:76`). Pairs with the v1.9 word-timestamp substrate above. *(transcription.rs:283–339, diarization.rs:77–115)*
  - [ ] **Expose `PipelineConfig` tunables in Settings** *(refined)*. speakrs exposes `merge_gap`, `speaker_keep_threshold`, `reconstruct_method`, and nested `binarize` / `ahc` / `vbx` configs (`pipeline/config.rs`). Caveat: `OwnedDiarizationPipeline::run` uses the pipeline's `default_config`; applying custom values needs `run_with_config` / `into_queued_with_config` / `new_with_config`. **ExecutionMode has no `CpuFast`** — the only `*-fast` modes are `CoreMlFast` / `CudaFast` (`inference.rs:47`), neither available on Windows/CPU — so ship the `merge_gap`/threshold knobs, not a Cpu/CpuFast toggle. *(pipeline/config.rs, inference.rs:47)*
  - [ ] **Selectable local diarization models** — speakrs ships
    `PipelineBuilder::from_dir(models_dir, mode)` beside the `from_pretrained`
    we call today, so custom/alternative model bundles are fully supported by
    the dependency. Ship in two steps: (1) `diarization.models_dir` directory
    override (replaces the dead `local_model_path` key — it was never wired to
    anything; Settings field + Doctor check follow the same resolution), so
    power users can drop in any compatible segmentation/embedding/PLDA bundle;
    (2) curated alternative bundles in the wizard (which wespeaker/pyannote
    ONNX exports actually match speakrs' shapes needs testing first), pinned
    SHA-256 like every other download. The diarizer cache invalidates on the
    new key like any `[diarization]` change.
  - [ ] **Speaker embeddings for named speakers** *(refined)*. `DiarizationResult` does expose `embeddings: ChunkEmbeddings(pub Array3<f32>)` and `hard_clusters: ChunkSpeakerClusters(pub Array2<i32>)` (`pipeline/types/data.rs:154`), so per-name centroids are computable. Caveat: `run_local_diarization` currently throws the whole result away except segments, and speakrs computes centroids internally (`pipeline/clustering.rs`) without a public accessor — we'd aggregate chunk embeddings per cluster ourselves, persist centroids per name, and cosine-match on later recordings. Real but non-trivial; lands after the scaling + caching fixes. *(diarization.rs:159–169)*
  - [x] **Cloud diarization toggles** ✓ *(shipped — backend AND Settings UI)*. Deepgram passes `diarize=true` and reassembles `[Speaker N]` from word speaker tags (transcription.rs:469, 521–553); AssemblyAI passes `speaker_labels=true` and reassembles from utterances (transcription.rs:702, 730–756). Both are gated on `DiarizationBackend::Deepgram` / `::Assemblyai` (transcription.rs:114, 122; config.rs:83). The **Settings UI** also shipped (commit `3e284b5`): the Speaker Diarization section offers all four backends (`none`/`local`/`deepgram`/`assemblyai`) with provider-conditional help boxes and a live mismatch warning (`SectionDiarization.ts`) that flags when cloud diarization is picked but a different provider transcribes. Round-trip + options + warning covered by `SectionDiarization.test.ts`.
  - [ ] **DER eval harness** *(refined)*. speakrs ships DER utilities — `compute_der`, `DerResult`, `parse_rttm` (`metrics.rs`), and `to_rttm` (`segment.rs`) — **but they are behind the `_metrics` feature**, which Phoneme does not enable (`speakrs = "0.4.2"`, default features in `phoneme-core/Cargo.toml:26`). Add a small RTTM fixture set + a dev-only harness that enables `speakrs/_metrics` (or reimplements collar-0 DER), wired as an optional nightly CI job rather than a release gate.
- [x] **Named speakers** — rename "Speaker 1" → "Sarah" once, persisted
  (`speaker_names` table) and rewritten into the transcript text itself, so exports
  and the merged view both carry the name; re-renamable after the fact. *(Manual
  rename shipped; automatic recognition via speaker embeddings is the separate
  item above.)*
- [ ] **Meeting capture profiles** — one click "Standup" (tag + summarize preset + Obsidian hook) vs "Interview" (diarize on, different prompt). Config profiles exist; tie them to capture intent.
- [ ] **Post-meeting digest** — meeting ends → optional "Summarize now?" with a one-click LLM preset.

### 🔎 Recall

- [x] **Show semantic relevance scores in the list** — hybrid search now returns a
  calibrated 0–100% relevance per hit (`fusion.rs::calibrate_cosine`) and the
  recordings list renders it as a chip during a semantic query.
- [ ] **"More like this"** — open a recording → find semantically similar ones. Nearly free: search by an existing recording's stored vector instead of a fresh query embedding. (Promoted from "medium" — embeddings already exist.)
- [x] **Saved searches / smart filters** — saves capture the *complete* filter
  state (query, kind, tags, dates, favorites, semantic mode), applied from the
  header dropdown and managed in Settings → Managers (also `g S`).
  (`SavedSearches.ts`, `SectionSavedSearches.ts`, `state/savedSearches.ts`)

### 🎙️ Dictation & capture feel

- [x] **Dictation fast lane** — *shipped*. In-place dictation runs its own
  minimal pipeline: skips the inbox queue entirely, transcribes with its own
  (optional) STT override, polishes with instant rule-based cleanup / an LLM /
  nothing, and types or pastes at the cursor — library bookkeeping happens
  after the text lands. Optional `full_pipeline` routes the dictation through
  the normal pipeline instead, and **`type_first`** picks *when* the text
  lands: immediately from a type-only fast pass (cleanup/summary/tags catch up
  in the library), or only after the full pipeline finishes. `[in_place]` in
  the config reference; user guide: `transcribe_in_place.md`.
- [ ] **Live preview overhaul (a whole phase)** — execution scope, concrete:
  **(a)** token-bucket reveal — words stream in at a steady cadence instead of
  replace-per-tick jumps; **(b)** stable stitch — committed text is append-only
  (provisional tail styled lighter, never rewriting what's already shown);
  **(c)** idle behavior — hold + gentle decay when the speaker pauses, a
  "listening" state instead of flicker; **(d)** per-tick perf budget with
  adaptive window (skip a tick under load rather than stutter); **(e)** exit
  criteria for dropping the Beta label: measured stitch stability + pacing on
  a 10-minute dictation. Original framing: the current streaming
  preview works but doesn't feel good: caption pacing is uneven, the stitch
  point jumps, and meetings double the cost. It ships **off by default,
  labelled Beta** until this lands. Scope: smoother partial-text pacing
  (token-bucket reveal instead of replace-per-tick), stable stitch at the
  window boundary, smarter idle behavior, and a real perf budget per tick.
  Wispr Flow ships NO live preview at all — that's how hard this is; ours has
  to feel right or stay off.
- [ ] **Waveform capture overlay** — a small bottom-center pill while
  dictating/recording showing the LIVE waveform of your own speech (plus
  state: listening / transcribing / ✍ typed). The interactive "it hears me"
  feedback Wispr Flow nails. Builds on the existing overlay window; do this
  AFTER the live-preview overhaul (or independent of it — the waveform needs
  only audio levels, not transcription).

- [ ] **In-place dictation, phase 2** — the fast lane shipped; now the feel:
  **(a)** voice commands in the polish pass — "new line", "new paragraph",
  "scratch that" handled rule-based in `fast_polish` (and as prompt directives
  in LLM mode); **(b)** per-app overrides — type vs paste vs off per process
  name (some apps reject synthetic keystrokes; the fast lane should know);
  **(c)** app-aware context, tier 1 — opt-in (OFF by default), the focused
  window's title feeds the polish prompt so jargon resolves correctly, with a
  process denylist; fully open source, toggleable — our answer to Wispr Flow's
  screenshots without the trust problem (tier 2, screenshot→vision-LLM, stays
  a separate later opt-in); **(d)** streaming-type experiment — type words as
  they finalize instead of all-at-end (spike: corrections vs cursor churn;
  may not survive contact with reality); **(e)** the waveform capture overlay
  above doubles as the dictation "it hears me" signal — build them together.

### ✨ Small wins

- [x] **Auto-generated titles** — *shipped* (heuristic on by default, optional LLM, user titles always win, click-to-edit). Was: timestamped names don't scan. Ship the **first-line/keyword heuristic first** (no dependency); LLM-generated titles as an *optional* enhancement (requires a configured LLM + adds latency).
- [x] **SRT / VTT export** — *shipped* (`phoneme export --captions <id> --format srt|vtt`). Was: captions for a Loom/YouTube clip from an imported file.

---

## 📋 v1.10 — Local Intelligence

**Theme: make Recall a moat.** Bigger, model-touching work that builds on v1.9.

- [x] **Transcript chunking + hybrid search** — *shipped.* Transcripts are split
  into overlapping, sentence-aware chunks (`chunk.rs`), each embedded into the new
  `embedding_chunks` table (migration `…_add_embedding_chunks.sql`); a recording is
  scored by its best-matching chunk (max-sim). The vector and FTS5 rankings are
  fused with Reciprocal Rank Fusion (`fusion.rs::reciprocal_rank_fusion`) in
  `catalog.rs::hybrid_search`, and raw cosine is calibrated to a 0–100% relevance
  for display (`calibrate_cosine`). The legacy one-vector-per-recording `embeddings`
  table is kept as a fallback until the background re-embed pass backfills chunks.
  *Still open:* a CI job that can run the ONNX model. *(The in-memory
  embedding cache shipped — the decoded corpus is held in RAM, invalidated
  on any re-embed/delete, bounded for large libraries.)*
- [ ] **"Ask my archive" (local RAG chat)** — "What did we decide about the API
  redesign?" → answer with citations/links to recordings. Builds on chunking +
  retrieval; needs a chat UI + citation UX. The headline differentiated feature
  *and the retrieval foundation for the Phoneme Agent below — ship this first,
  then the agent turns its retrieval into one tool among many.*
- [ ] **Phoneme Agent** — a fully phoneme-aware agent living right inside the
  app: a chat panel that doesn't just *answer* from the archive but *acts* on
  it. "Find every standup from this week, tag them `standup`, and give me the
  open action items" — it searches (hybrid/semantic), reads transcripts, then
  tags, titles, summarizes, re-runs steps, exports captions, starts/stops
  recordings, and adjusts filters, chaining steps until the job is done.
  - **Repo decision (made):** built IN THIS WORKSPACE — `crates/
    phoneme-agent-core` (the loop + tool registry, reusing the existing LLM
    provider stack) + a Lit chat panel. The tool layer tracks the Request
    enum compiler-enforced; a separate repo would version-skew against a
    wire contract that changes weekly. A standalone TUI agent (opencode's
    form factor) can become its own repo LATER as a thin MCP/REST client.
  - **Don't write the harness from scratch.** Adapt an existing open-source
    agent harness instead: **opencode** (free, open source) is the named
    candidate — take its agent loop (provider abstraction, tool-calling loop,
    permission gating, session/replay) and bend it to Phoneme. Evaluate at
    build time against two lighter alternatives: the Vercel AI SDK's
    tool-loop (TS, fits the Lit frontend) and **Rig** (Rust-native, fits the
    daemon if the brain lives backend-side). License + maintenance check is
    part of the pick; the decision record lands in docs/design/.
  - **Tool layer = the IPC surface we already have.** Tools are typed wrappers
    over the existing `Request` enum via the `Transport` trait — the same thin
    layer the v2.0 MCP server and REST API translate. Write the tool registry
    once; the in-app agent, MCP (external agents), and REST all consume it.
  - **Trust model, local-first:** runs on any configured LLM with tool calling
    (local Ollama models included — curated "agent-capable" picks in the model
    field); every tool call renders in the chat as a visible step; destructive
    ops (delete, bulk edits) require an in-chat confirm and route through the
    existing undo paths; a per-session action log makes everything auditable.
    No screen access, no shell — its hands are the app's own IPC, nothing else.
  - **UI:** a right-side companion panel (g-chord + header button), streaming
    responses, citations linking into recordings like Ask-my-archive, tool
    steps collapsible like the Doctor's passing checks.
  - Sequencing: needs Ask-my-archive's retrieval + citation UX, the unified
    provider/model picker (shipped), and the granular status/event plumbing
    (shipped). Pairs naturally with the v2.0 MCP server — same tool registry,
    opposite direction.
- [ ] **Transcript ↔ waveform sync** — click a paragraph → seek playback. *(Needs
  word-level timestamps from v1.9.)*
- [x] **Compare transcript versions** — side-by-side diff of original Whisper vs
  cleaned vs the current edit (`TranscriptDiff.ts`); audited clean.
- [ ] **Custom vocabulary / glossary** — names like "Phoneme", "pyannote", client
  acronyms transcribed correctly via Whisper's `initial_prompt`. (Dictator persona,
  Whisper-native.)
- [x] **Auto-tag suggestions** — *shipped* as a full pipeline stage with
  approve/dismiss UX, its own `[auto_tag]` provider config, and auto-accept for
  tags the library already has. Smart **titles** are the remaining half — tracked
  as the v1.9 auto-generated-titles item.
- [x] **Transcription queue dashboard** — the queue panel now shows pending /
  processing / failed (badge + clear), supports reorder, pause, cancel, and
  skip-current-stage, and pins the active item. *Still open:* per-file error
  detail + one-click retry on failed entries.
- [ ] **Per-recording hook override** — this one goes to Discord, that one stays
  local. (Today hook config is global; re-fire is manual.)
- [ ] **Confidence highlighting** — low-confidence words underlined; click to fix.
  *(Needs word-level probabilities + segment storage — pairs with v1.9 word-level
  infra.)*

---

## 🔮 v2.0 — Platform & Integration

*Focus: cross-platform availability and opening Phoneme to external tools.*

### Platform
- [ ] **macOS port** — Apple Silicon first; bundled whisper.cpp server. Ship microphone-only first; do NOT let Meeting Mode block the macOS launch. `cpal` has no system-audio loopback on macOS — it requires a virtual device (BlackHole / Loopback). So on macOS: mic capture works natively; system-audio capture is opt-in via an external loopback device the user installs. Treat full feature parity as a follow-up, not a launch gate.
- [ ] **Linux port** — PipeWire / ALSA audio (PipeWire monitor sources give system-audio loopback natively, unlike macOS); X11 + Wayland global hotkey.
- [ ] **Windows ARM** — native ARM64 build for Snapdragon-based machines.

### Integration

> **Architecture decision (locked):** the daemon already speaks newline-delimited JSON over a named pipe behind the `phoneme-ipc` `Transport` trait. v2.0 adds an **HTTP front-end, not a new eventing model**: an `axum` server maps one-off `Request`s to REST endpoints (`POST /api/record/start`, `GET /api/recordings`) and streams `DaemonEvent`s as **Server-Sent Events** (`GET /api/events`, an `EventSource` in the frontend). REST API, browser extension, Raycast scripts, and the MCP server then all share one `fetch()`/`EventSource` surface.

- [x] **Local REST API** — `localhost:3737` `axum` server (off by default): REST endpoints over the existing `Request`/`Response` enums + an SSE `/api/events` stream over `DaemonEvent`. Add an `HttpTransport` impl of the `Transport` trait so clients reuse the same typed surface. *Shipped:* `bin/phoneme-rest`, `127.0.0.1`-only, off by default (`[rest_api] enabled`, port 3737), endpoints over `Request`/`Response` + SSE `/api/events`, with a bounded daemon round-trip (a wedged daemon → 503, never a hung request). The client-side `HttpTransport` impl is deferred — the server is the deliverable. See `docs/developer-guide/rest_api.md`.
- [x] **MCP server** — `phoneme-mcp` binary (MCP = JSON-RPC over stdio). A **thin translator over the existing `Transport` trait**: `CallTool("start_recording")` maps to `Request::RecordStart` — near-zero business logic. Tools: `start_recording`, `stop_recording`, `get_transcript`, `search_recordings`, `list_recent`. Shares the tool registry with the v1.10 **Phoneme Agent** — same typed wrappers, opposite direction (external agents in, instead of the in-app agent out). *Shipped:* observe-only (never spawns a daemon), bad input + a down/wedged daemon surface as MCP tool errors, and the stdio framing is bounded (8 MiB) like the IPC codec. See `docs/developer-guide/mcp_server.md`.
- [ ] **Webhook improvements** — HMAC-SHA256 signing; configurable trigger point (before hook, after hook, or independent); custom headers. *Partly shipped:* HMAC-SHA256 signing (`[webhook] hmac_secret` → `X-Phoneme-Signature: sha256=<hex>`) and `[webhook] custom_headers` are done; the configurable trigger point is still open.
- [ ] **Browser extension** — toolbar icon; one click starts a recording and pastes the finished transcript into the focused field or clipboard. Requires the v2.0 REST API as the bridge.

### Recording
- [ ] **Multi-microphone** — capture from two input devices simultaneously (two-person interviews).
- [x] **Audio normalization** — peak-normalize a quiet recording's gain before Whisper; improves accuracy on soft microphones. *Shipped:* off by default (`recording.normalize`, `-1.0` dBFS ceiling), boost-only, applied to the finalized single recording and each meeting track (never the live preview or imports).

### Data
- [ ] **Cloud sync** (opt-in, user-controlled) — encrypted sync of the catalog to a user-owned S3/Backblaze bucket; audio files excluded by default.

### Internal Quality
- [ ] **Playwright E2E UI coverage** — full E2E suite driving the frontend against the real Rust backend over IPC. After the architecture stabilizes across macOS/Linux.

---

## 🌌 Long Term

*No fixed timeline. Require significant platform work or community infrastructure.*

- [ ] **Mobile thin-client** — iOS/Android records locally, syncs to the desktop daemon over LAN; transcription runs on the desktop.
- [ ] **Plugin ecosystem** — standardized API for community hooks/themes/post-processors via a JSON registry.
- [ ] **Phoneme Cloud** (optional, self-hostable) — shared catalogs + role-based access for teams; the desktop daemon stays fully offline-capable.
- [ ] **Accessibility pass** — full NVDA/JAWS support, ARIA labels, font scaling, high-contrast theme.

### Architecture evolution
- [ ] **Protocol versioning** — version field on IPC Request/Response.
- [ ] **Batch operations** — batch delete / batch tag update.
- [ ] **Priority queue**, **parallel processing**, **health endpoint**, **metrics export**, **`--status` flag**.

### Data model
- [ ] **DB maintenance** (vacuum strategy), **indexing strategy** for 100k+ catalogs, **phrase search** (quoted FTS5).

### Security
- [x] **Content Security Policy** + **scoped permissions** — *shipped with the v1.8.x CSP/scopes hardening above.*

---

## 💰 Sustainability & Monetization

*Revenue ideas that keep the core desktop app 100% free, local, and open-source.*

- [ ] **Paid Mobile Companion App** — one-time fee / micro-subscription thin-client that records on the go and syncs back to the desktop daemon.
- [ ] **Phoneme Pro (Managed APIs)** — optional $8–10/mo for zero-config cloud Whisper + premium LLM cleanup without managing API keys.
- [ ] **Phoneme Sync** — $4–5/mo end-to-end-encrypted sync of the SQLite catalog + audio across machines (Obsidian-Sync-style).
- [ ] **Phoneme for Teams** — per-seat managed backend; centralized, searchable, role-governed meeting notes.

---

## ❌ Explicitly Not Doing

Considered and rejected — so we don't revisit them. (Speculative ideas that *might*
graduate someday live in the [Idea Parking Lot](docs/IDEAS.md) instead.)

| Idea | Reason |
|------|--------|
| Duration filter | Niche; nobody asked; search + tags already narrow the list |
| Backup/restore ZIP | Manual export covers it; the SQLite DB is a single copyable file |
| Azure Speech / AWS Transcribe | Enterprise pricing; not the target user; add only on demand |
| Portable (unsigned) ZIP | A CI task, not a product feature |
| Winget / Scoop packages | Same — packaging automation, not a roadmap item |
| Meeting-app awareness (auto-detect Zoom/Teams) | Brittle, false-positive-prone, and surveillance-y for a privacy-first app |
| Voice commands / wake word | Push-to-talk already solves hands-free; wake-word is a false-trigger rabbit hole |
| Transcript git-style version graph | YAGNI at this scale; original-vs-current diff covers ~95% |
| Acoustic echo cancellation (speaker→mic bleed) | Genuinely hard research problem; honest answer is "wear headphones" |
| Word-by-word streaming transcription | Moved to the v2.0 backlog; the bounded live preview covers the need for now |

---

## 🔬 Audit follow-ups (June 2026 code audits)

Two line-by-line audits (a combined module pass + a net-new pass on the newest
code). **Not started** — these land in waves once kicked off. *Already resolved:* DPAPI
at-rest, the masked-config WebView boundary, diarization coalescing, list-pane
fill + scroll-extend, the detail-pane overhaul. The transcript-diff,
saved-searches, and curated-models features audited **clean**.

**Wave 1 — High (correctness & security)**
- [x] whisper-server stdout/stderr never drained → pipe fills (~64 KB) → hung transcription / false timeout — both spawn sites now use `Stdio::null()` (`whisper_supervisor.rs`) *(A2-H1)*
- [x] `native-whisper` won't compile — the Option pattern-match on the String `model_path` is fixed (cfg'd-out code is never type-checked, which hid it). Building the feature still needs LLVM locally (see building_from_source), so it stays out of CI. *(A2-H2)*
- [x] tray `Bridge` stays `None` after a down-at-launch daemon — replaced by a lazily-reconnecting `BridgeSlot`: the first action retries the auto-spawn + connect, the event stream attaches in the background the moment the daemon appears, and the startup chrome (titlebar, hotkeys, overlay) no longer depends on the bridge at all. *(A2-H3)*
- [x] `wizard_download_model` now uses the same https host allowlist as `wizard_download_file`; `wizard_run_installer` canonicalizes both sides of the temp-dir check (lexical starts_with let `..` through) and only runs `.exe`. *(A2-H4/H5)*
- [x] Delete key sends `session:<id>` to `deleteRecording` — fixed by the undoable-delete flow (`requestUndoableDelete` filters session ids) *(A1-H1)*
- [x] PostProcessing cloud `/models` fetch sends the masked sentinel key — `fetchLlmModels` guards the sentinel (cloud → manual entry, local → blank key) *(A1-H2)*
- [x] `open_file` is restricted to the audio library, phoneme's data dir, and its config dir (canonicalized), mirroring `reveal_file`. *(A1-H3)*
- [x] Import enqueue failure orphans a catalog row — the row and canonical WAV roll back so a failed import is simply retryable *(A1-H4)*
- [x] Whisper transient failure never requeues — unreachable/timeout now skip failed/, the worker requeues the same item with backoff (capped at 5 attempts) *(A1-H5)*. *Model-override readiness race (A2-M7) still open — Wave 2.*

**Wave 2 — Perf & UX correctness** — embed read-lock contention + `spawn_blocking` + diarizer pipeline cache (A2-M8), cancel → distinct status (not `TranscribeFailed`), meeting-stop best-effort per track (A2-M6), poisoned model download (A2-M1), per-request retranscribe override (A2-M21), server-side `kind` filter for sparse pages.

**Wave 3 — CLI / doctor / config** — `config set` atomicity + validate + resolved path (A2-M3), `status` without auto-spawn (A2-M4), doctor resolved-path + per-provider probes (A2-M5/M15), preview-config validate/expand (A2-M13/M14), pipe-busy connect deadline (A2-M9). ~~Fix stale `ActionRow.test.ts` (A2-M22)~~ — done (rewritten against the merged Re-run form).

**Wave 4 — Hardening & data integrity** — queue crash-dup window (A2-M12), retention in a transaction (A2-M20), U16 capture path (A2-M10), import OOM cap (A2-M11), bounded LLM/webhook error bodies (A2-M16/M17), profile-switch re-registers all hotkeys (A2-M18), overlay capability split (A2-M19), webhook SSRF guard + queue-IPC integration tests.

**Wave 5 — Low / docs / DX** — ~25 low-severity items + doc drift (CHANGELOG v1.8.x, smoke-test steps, `building_from_source` LLVM, `frontend/README`), populate `docs/screenshots/`, a `config validate` CLI, ESLint in CI. See the audit doc.

---

## 🧰 Engineering & tech-debt backlog

*Not user-facing features — internal quality work, pulled in opportunistically
alongside the feature releases above.*

**Reliability**
- [ ] Retry/backoff for webhooks; rate limiting / circuit breakers for external services (OpenAI, Ollama, webhooks).
- [x] Reconnection backoff/limit in `bridge.rs` — the `BridgeSlot` reconnect path now rate-limits with bounded exponential backoff (250ms → 10s cap), so a burst of UI actions during a daemon outage no longer spawn-storms. Cap-and-keep-trying-slowly: a successful connect resets it, and a daemon started later still heals once the window elapses (no hard give-up).
- [ ] Replace remaining `unwrap()` in production paths (`recorder.rs` source opens; remaining hot paths).
- [ ] Integration tests for daemon components; a synthetic-audio E2E covering the single-recording path.

**Doctor**
- [x] Restart/fix for the local whisper servers (`RestartWhisper` sweeps strays +
  bounces both supervisors), a header health pill + failure banner, `phoneme
  doctor --fix` on the CLI, and Doctor in the main nav (`g D`).
- [x] Disk-space + model-integrity checks; check categories (Critical/Warning/Info); per-check explanations + fix guidance; "Fix All". — *shipped.*

**Code organization**
- [ ] Split the large files (`config.rs`, `catalog.rs`, `recorder.rs`, `commands.rs`) into modules; dedupe `auto_spawn.rs` (CLI + Tauri); move `grouping.ts`/`form.ts` to `utils/`.
- [x] Frontend: ESLint + Prettier (*shipped — flat config, 0-error baseline, lint in CI*); still open: stricter TS (`noUnusedLocals`/`noUnusedParameters`); `types/` + `constants/` dirs.

**Performance**
- [ ] **Saved searches in the catalog** — today they live in webview
  localStorage only (lost on profile reset/reinstall, invisible to the CLI
  and `phoneme export`). Migration `saved_searches` (id, name UNIQUE COLLATE
  NOCASE, filter_json, timestamps) + List/Upsert/Rename/Delete IPC +
  `saved_searches_changed` event; one-time localStorage import on first GUI
  run; store tag NAMES beside numeric ids so snapshots survive export/import;
  CLI `phoneme list --saved <name>`. Effort S/M — the UI already isolates
  storage behind `state/savedSearches.ts`, and the rename-conflict API
  carries over as the fast path.
- [x] Toast cosmetics: failure toasts strip the thiserror "internal error:"
  prefix; TranscriptDiff computes the (capped) diff once per refresh now.
- [x] Persist `error_kind`/`error_message` onto the catalog row at failure
  time — both failure paths now write them (a retry clears them); failure
  reasons survive a restart and the queue panel's cache is the live-event
  fallback.
- [ ] Per-item failed-quarantine dismiss — the inbox failed/ store only
  supports all-or-nothing ClearFailed; the failure panel wants per-recording
  dismiss IPC.
- [x] Settings/wizard URL hints show the EFFECTIVE whisper port — after a
  fallback the Transcription/Dictation hints and the wizard's preview note
  name the bound port ("running on 51234 — preferred 5809 was busy").
- [x] Doctor probes follow effective whisper ports — both the daemon-side
  RunDoctor handler and the separate tray-side backend checks rewrite the
  local-bundled probe URL via the published effective port and say "running
  on 51234 (fallback from 5809)" when it differs from the configured one.
- [x] Record the request model id for cloud STT — cloud/custom backends now store the requested `whisper.model`; local keeps the file-stem.
- [ ] Doctor: decide whether local whisper model/server checks should skip (or downgrade) when a cloud STT provider is configured — today they run regardless (behavior parity kept on purpose).
- [ ] Trim redundant `http.clone()` (transcription.rs ×7, llm.rs ×4); avoid the `attention_mask` clone in `embed.rs`.

**Docs / DX**
- [~] `config.example.toml` + `.env.example`; document JSON output + env vars; semantic-search + advanced-search-syntax docs; troubleshooting (audio devices, network/cloud, performance).
- [x] Shell completions — `phoneme completions <shell>` (bash/zsh/fish/powershell/elvish).
- [ ] `cargo-audit`/`cargo-deny`; code coverage. (Stale `release_notes.md`/`.txt` scratch files removed — GitHub releases auto-generate notes.)

**Testing & CI** *(from June 2026 audit — ~6/10 maturity; strong Rust foundation, integration gaps)*
- [x] **Gate `release.yml` on `cargo test` + vitest** — a `test` job (fmt + clippy + cargo test + vitest + type-check) now blocks the release job.
- [ ] **Pipeline integration tests** — the full transcribe → LLM → hooks → webhook → catalog/inbox path is the biggest untested critical path (`pipeline.rs`).
- [x] **Webhook + embedding tests** — `webhook.rs` timeout/error contracts; embedding upsert/search round-trip + corrupt-BLOB handling. *(shipped in PR #68)*
- [ ] **Meeting capture E2E** (synthetic backend, incl. a paused-video internal gap) and **retention daemon** + **export-zip** integrity tests.
- [ ] Split CI: parallel `--lib`, serial `--test '*'` (today everything runs `--test-threads=1`); cache the `tauri-cli` binary.

**Quick correctness wins** *(small, mostly isolated)*
- [x] `embed.rs` mutex-poison `unwrap()` → `map_err` (no daemon panic on a poisoned lock). *(audit A-C2)*
- [x] Atomic `toggle_meeting()` — holds a `toggle_guard` across the read+act so concurrent toggles serialize. *(A-H11)*
- [x] Shared `Config::load_resolved()` so the CLI honors `PHONEME_CONFIG` like the daemon; shared `register_hotkeys(cfg)` so startup and a profile switch re-register *all* hotkeys (record/meeting/in-place). *(A-H13/A-H14)*
- [x] Structured Tauri IPC errors (`{ kind, message }`) instead of flattened strings; frontend reads them via `errText`/`errKind`. *(A-H6)*
- [x] Align `zip` versions across workspace vs tray (tray now uses the workspace `zip`). *(A-H12)*
- [x] Delete or implement the orphaned `checkMicrophoneAccess()` (no Tauri handler). *(A-C3)*
- [x] Consolidate the duplicated **Doctor** and triplicate **record-mode** enums into core. *(A-H3/A-H4)*

**Docs accuracy** *(audit found drift — fix the user-facing claims)*
- [x] Say **speakrs**, not "Pyannote", everywhere (docs + `SectionDiarization.ts`). *(A-C5 — verified clean.)*
- [x] Reconcile claims that don't match code:
  `hook.log` / `HookPayload.original_transcript` are no longer claimed; the hook
  payload doc matches the struct; the merged-meeting and semantic-search docs were
  rewritten to match the shipped coarse merge + chunked hybrid search; `docs/screenshots/`
  is populated. Validation is automatic on load/reload (no `phoneme config validate`).

---

*Last reorganized around the four-persona model + a backend-ahead-of-GUI audit.
Pick a target version, decide whether "finish the attic" is one release or
background polish, and ship one medium feature (merged timeline or auto-titles)
per cycle.*
