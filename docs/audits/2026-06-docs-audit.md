# Documentation audit — June 2026

A full cross-reference of the docs against the current code (branch
`feat/v20-queue-columns`, workspace `1.8.1`). The codebase had drifted ahead of
the docs: several recently-shipped features were either undocumented or still
described as "planned," and a few docs described behaviour the code no longer has.

This note records what was stale and what changed, per file. Markdown-only edits —
no code touched.

## Headline findings

1. **ROADMAP listed shipped features as not-done.** Chunked hybrid semantic search,
   embedding-model choice, the merged meeting view, the system-wide overlay, masked
   config (S-H2), IPC connection resilience, queue failed-count, semantic relevance
   chips, and the Import-audio button were all unchecked `[ ]` despite being in the
   code and in `git log`. These were the biggest promise-vs-reality inversions.
2. **Merged meeting view was mis-described.** `meeting_mode.md` and `CHANGELOG.md`
   described the merged view as separate per-track editors / a *chronologically
   interleaved* "You / Meeting" transcript. The shipped feature
   (`MergedConversationDetail.ts` / `mergeMeeting.ts`) is a **coarse,
   source-sectioned, speaker-aware, read-only** merge — true time-interleaving is
   explicitly *not* done (per-line timestamps aren't persisted). See
   `docs/design/merged-meeting-view.md`.
3. **Semantic-search docs were a generation behind.** They described
   one-vector-per-recording cosine search and a "Settings → System → Advanced"
   location. Reality: per-chunk embeddings (`embedding_chunks`), RRF hybrid fusion
   with FTS5 (`fusion.rs`, `catalog::hybrid_search`), calibrated relevance, a
   user-choosable embedding model, and a dedicated **Semantic Search** settings
   section + **Re-embed** action.
4. **Threat model still listed S-H2 (masked config) as open.** The WebView-boundary
   masking is implemented (`commands.rs` `mask_config_secrets`/`unmask_config_secrets`);
   only encryption-at-rest (DPAPI) remains.
5. **A broken Mermaid diagram** in `meeting_mode.md` reused the node id `M` for both
   "Mic" and "Merge," which collapses the two nodes and mis-wires the graph.

## Needs a human decision

- **Semantic Search settings have no tab.** `SectionSemantic.ts` is only mounted in
  the Settings *search* path (`SettingsView/index.ts` `mountAll`), never in a tab's
  `switch`. So the embedding-model controls and the **Re-embed all recordings**
  button are reachable only by typing "Semantic" into Settings search. Docs now say
  so, but this is almost certainly a wiring oversight — a one-line fix to mount it
  under the System or Transcription tab. *(Flagged as a follow-up task.)*
- **CHANGELOG version label.** The recent work is documented under a new
  "v1.8.x (in development)" section (workspace is `1.8.1`). If these landed under a
  different intended version tag, relabel that heading.

## Per-file changes

### `ROADMAP.md`
- Added a "Recently shipped (this cycle)" block (chunked hybrid search, embedding
  model choice, merged view, overlay, S-H2 masking, IPC resilience, queue
  failed-count + Import).
- Ticked: S-H2 masked-config (secrets-half), failed-queue visibility+clear (noting
  retry is still pending), semantic-search settings+re-index, import file picker,
  semantic relevance scores, and the v1.10 "transcript chunking + hybrid search"
  item (shipped early). Marked the merged-timeline item `[~]` (coarse merge done;
  chronological interleave pending).
- Rewrote the stale "Docs accuracy" tech-debt item: verified the speakrs/Pyannote,
  `hook.log`, and `HookPayload.original_transcript` claims are already clean, and
  pointed it at this audit.

### `CHANGELOG.md`
- New "v1.8.x — Recall, Meetings & Hardening (in development)" section.
- Corrected the v1.7.1 "merged conversation view" entry — it claimed chronological
  timestamp interleaving; reworded to the coarse merge actually shipped.

### `docs/user-guide/semantic_search.md`
- Rewritten: chunking, hybrid RRF, calibrated relevance, the embedding-model knobs
  table (`max_tokens`/`pooling`/`token_type_ids`/prefixes), the Re-embed action, and
  a corrected enable path ("search Settings for 'Semantic'"). Refreshed the
  comparison table and troubleshooting.

### `docs/user-guide/search_and_organization.md`
- Updated the semantic blurb to describe chunked hybrid search + relevance chip and
  the corrected settings location.

### `docs/user-guide/meeting_mode.md`
- Corrected the merged-view description (single read-only sectioned reading with
  Copy/Export, not per-track editors / chronological interleave).
- Fixed the Mermaid node-id collision (`M` → `MG` for the Merge node).

### `docs/user-guide/settings_overview.md`
- Added the **Import audio** button to Storage; removed the stale "semantic-search
  model path" from Advanced and added a **Semantic Search** subsection (with the
  "reachable via search" note); mentioned the **System-wide overlay** checkbox under
  Live Preview.

### `docs/user-guide/streaming_preview_and_preroll.md`
- Described the bounded ~15 s rolling window + forward-growing stitch, the
  whisper-permit yield, the ~2 s / ~1 s-native cadence, the `[preview_whisper]`
  option, and a new **System-wide overlay** subsection. Noted the preview follows
  the mic track in meetings.

### `docs/developer-guide/config_reference.md`
- `[semantic_search]`: added `max_tokens`, `pooling`, `token_type_ids`,
  `query_prefix`, `passage_prefix` + a re-index note.
- `[interface]`: added `preview_overlay`.

### `docs/developer-guide/threat_model.md`
- Moved the masked-config DTO (S-H2) into "Mitigations in place"; trimmed the open
  item to encryption-at-rest only.

### `docs/developer-guide/internals.md`
- Catalog: `embedding_chunks` is now the primary store; `embeddings` is the legacy
  fallback. Listed all five migrations.
- New "Semantic search" subsection (`chunk`/`embed`/`fusion`/`hybrid_search`).
- IPC: documented `ServerRequest::Unknown` connection resilience and `reembed_all`.
- Frontend: documented the `overlay.html`/`overlay.ts` second entry.

### `docs/developer-guide/architecture.md`
- Data-model bullet updated to `embedding_chunks` + hybrid search.

### `docs/developer-guide/data_directories.md`
- Catalog tree + schema table updated to `embedding_chunks` (primary) + legacy
  `embeddings`.

### `docs/IDEAS.md`
- "Live meeting subtitles overlay" → status *partially shipped* (the overlay exists;
  true real-time captioning remains). Updated the duplicate-detection prerequisite
  (chunked embeddings now exist).

### `README.md`
- Core-features semantic bullet now describes chunked hybrid search + BYO embedding
  model.

### `crates/phoneme-core/README.md`
- Expanded the responsibilities list to the actual module set (transcription,
  diarization, chunk/embed/fusion, doctor).

### `crates/phoneme-ipc/README.md`
- Added a "Robustness" section: `ServerRequest::Unknown`, owner-only pipe ACL, 8 MiB
  frame cap.

### `src-tauri/README.md`
- Module table: added `overlay`, the masking/`set_overlay`/`reembed_all` commands,
  and an S-H2 secret-handling note.

## Verified-accurate (no change needed)

- `docs/developer-guide/plugins_and_hooks.md` — the JSON payload table matches the
  `HookPayload` struct exactly and correctly states `original_transcript` is *not*
  in the payload.
- `docs/developer-guide/cli_reference.md` — `config` has `set`/`path`/`reload`
  (no `validate`); doc already notes validation is automatic.
- `docs/screenshots/` — populated (12 images); the ROADMAP's "empty screenshots"
  claim was itself stale.
- "speakrs" (not "Pyannote") is used consistently across docs and
  `SectionDiarization.ts`.
