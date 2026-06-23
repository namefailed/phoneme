# 💡 Idea Parking Lot

Speculative, unvetted, or deliberately-deferred ideas. **Nothing here is committed.**
This is where ideas wait until they earn a place on the [Roadmap](../ROADMAP.md) —
or get moved to "Explicitly Not Doing" if we decide against them.

Two things live here:
1. Ideas that came up in audits/brainstorms but aren't ready to plan.
2. Things pulled *off* the roadmap that we didn't want to fully reject — kept here
   so the reasoning isn't lost.

**To promote an idea to the roadmap**, it should clear the project bar:
*would a real user actually hit this friction, and does it serve more than a
sliver of users?* A cheap prototype or a concrete user request is the usual trigger.

---

## 🅿️ Parked — interesting, not yet justified

### Duplicate / near-duplicate detection
"You already recorded this call" when importing or starting a meeting.
- **Why parked:** needs embedding-similarity + time-overlap heuristics and a good
  false-positive story. The chunked-embedding substrate it depended on now exists
  (`embedding_chunks` + `catalog::hybrid_search`).
- **Promote when:** a user actually complains about dupes (the embedding prerequisite
  is met).

### Chapter markers
Auto-split a 90-minute meeting on long silences into navigable chapters.
- **Why parked:** silence detection already exists, but it's a fair bit of UI +
  export work for a need nobody's voiced yet.
- **Promote when:** someone records genuinely long sessions and wants navigation.

### Live meeting subtitles overlay
Floating captions during Meeting Mode without alt-tabbing.
- **Status: largely shipped.** A system-wide, always-on-top, frameless live-preview
  overlay floats the caption over any app (opt-in via `interface.preview_overlay`;
  `src-tauri/src/overlay.rs`, `frontend/overlay.*`), **and** a second independent
  always-on-top recording-indicator overlay (dot + waveform + timer;
  `src-tauri/src/indicator.rs`) works even with captions off. Both are
  **multi-monitor-aware** (per-label window-state geometry). The caption shows the
  **live preview** stream (a rolling re-transcription), not yet true word-by-word
  real-time captions.
- **Remaining:** only lower-latency, true word-by-word real-time captioning (this is
  the same thing as the "Word-by-word streaming transcription" item in ROADMAP →
  *Not convinced yet*). Multi-monitor and the indicator overlay are done.

### Team glossary sync
Shared names/terms (custom vocabulary) across machines/teammates.
- **Why parked:** depends on a sync story, which is a different shape from the
  opt-in S3 cloud-sync planned for v2.0. Single-machine custom vocabulary (v1.10)
  has to exist and prove useful first.
- **Promote when:** custom vocabulary ships and teams ask to share it.
- **Cheap prerequisite (not parked):** a plain **export/import file** for a glossary
  or a Playbook recipe sidesteps the sync question entirely — promote that first if
  anyone asks to share config.

### Smaller quality-of-life ideas
A grab-bag of low-cost ideas that don't yet clear the bar on their own:
- **WER / accuracy benchmark harness** — a dev-only "transcribe a reference clip →
  score against ground-truth" twin of the DER harness, so a model/provider swap can't
  silently degrade *what-was-said* accuracy. Promote if accuracy regressions actually bite.
- **Per-recording confidence badge** — roll the per-word probabilities (already used
  by the squiggle) into one "92% confident" chip + a jump to low-confidence spans.
- **Trim / crop dead air; Opus/MP3 re-export** — niche audio editing on top of the
  existing silence detector + decode pipeline; promote when someone reuses the audio
  outside transcription.
- **Disk-space pre-flight on record-start; background re-embed progress in the UI** —
  small ops guardrails (Doctor only warns after the fact today).
- **Push-to-talk max-duration safety cap** — a per-Hold limit separate from the 3 h
  absolute cap; one config knob, mitigates the dropped-key-up stuck-recording footgun.

### MCP / REST surface breadth
The CLI has near-total IPC parity, but **MCP exposes 32 tools and REST 24 routes**
against the daemon's much larger IPC request surface — still a slice, not the whole.
REST has since grown to cover meeting start/stop, queue, tags, title/favorite/pinned,
and cleanup/summary re-runs; the remaining gaps are the deeper edit/version ops, and
`REST record/start` is hold-mode-only.
- **Why parked here, not roadmapped:** the in-app **Phoneme Agent** (ROADMAP H2) and
  the **browser extension** (H2, rides REST) both *depend* on broader coverage, so the
  breadth work is best scoped *with* them rather than as a standalone item.
- **Promote when:** either of those is picked up, or an automation user hits the wall.

---

## 🛑 Deferred — a high bar to revisit

These were considered and pushed off the roadmap. They're **not banned** — like
everything in Phoneme, a real case can resurrect any of them (Favorites was a "no"
once) — but the bar is high, so they're parked with their reasoning rather than
re-litigated every quarter.

### Meeting-app awareness (auto-detect Zoom/Teams/Meet)
Detect a meeting app in the foreground → prompt "Start meeting capture?"
- **Why deferred:** brittle (process/window detection), false-positive-prone, and it
  feels surveillance-y for a privacy-first, local app. The cost/creepiness outweighs
  the saved click of pressing the meeting hotkey.

### Voice commands / wake word
"Stop recording", "tag this work", hands-free.
- **Why deferred:** push-to-talk + the meeting hotkey already solve hands-free
  control. Wake-word detection is a false-trigger and always-listening-privacy rabbit
  hole for marginal benefit.

### Acoustic echo cancellation (speaker → mic bleed)
Meeting Mode without headphones.
- **Why deferred:** AEC is a genuine signal-processing research problem. The honest,
  shippable answer is "wear headphones," and dual-track capture already separates the
  sources. Not worth the complexity.

### Transcript git-style version history
Branch/restore across many edits and re-transcriptions, not just original-vs-current.
- **Why deferred:** YAGNI at this user count. `original_transcript` (raw Whisper) is
  already preserved, and a simple **diff view** (planned for v1.10: original vs LLM
  vs manual edit) covers ~95% of the real need. A full version graph in SQLite + a
  history UI is a lot of machinery for the last 5%.

### Streaming stall indicator (mid-stream LLM heartbeat)
While a summary/cleanup streams token-by-token, a mid-stream provider stall (e.g.
Ollama's per-chunk idle timeout) just stops the deltas — the live UI can't tell a
"slow local model" from a "stuck" one. Real failures already surface through the
terminal `summary_failed` / `cleanup_failed` events, so this is polish, not a
correctness gap.
- **Why parked:** would add an error/heartbeat field to the `LlmActivity` IPC
  event (schema + daemon + frontend) so the streaming view can show a
  stall/timeout marker. Surfaced by a post-merge cluster audit.
- **Promote when:** users report streaming that "hangs" in a way they can't
  distinguish from a genuinely slow model.

---

*See also: [`ROADMAP.md`](../ROADMAP.md) → "Not convinced yet" for ideas we've
pushed back on for now (duration filter, backup ZIP, Azure/AWS STT, etc.) — none of
it permanent. (Favorites was once on that list; a real case appeared and it shipped.)*
