# Technical Challenges & Engineering Decisions

Phoneme is a local-first, real-time voice toolkit: it captures audio, streams a
live transcript, runs a configurable AI pipeline, diarizes meetings, and serves a
semantic archive — all from a single Rust daemon driving a Tauri desktop shell, on
a normal Windows box with no cloud dependency required.

Most of the interesting work isn't the happy path; it's the dozens of places where
the obvious implementation is correct in a demo and wrong in production. This
document collects those problems — the bug that only appears on a long recording,
the race that only fires during a restart, the security check that has to *not*
fire for the local-first case — and how each was solved. Every entry links the
real code.

> Each entry follows the same shape: **the problem**, **why it's subtle** (the
> part the naive implementation gets wrong), and **the solution**.

## Contents

1. [Real-time audio & live preview](#real-time-audio--live-preview)
2. [Transcription-engine supervision](#transcription-engine-supervision)
3. [Meeting capture & track alignment](#meeting-capture--track-alignment)
4. [Speaker diarization](#speaker-diarization)
5. [The processing pipeline & Playbook](#the-processing-pipeline--playbook)
6. [In-place dictation](#in-place-dictation)
7. [Search & recall](#search--recall)
8. [Security & the trust boundary](#security--the-trust-boundary)
9. [IPC & desktop reliability](#ipc--desktop-reliability)
10. [Data integrity & lifecycle](#data-integrity--lifecycle)

---

## Real-time audio & live preview

### The live-preview O(n²) blow-up

**Problem.** The streaming live-caption loop re-transcribed the *entire* growing
audio buffer every ~2 s, so per-tick cost grew with recording length — O(n²) over a
take. A heavy model on a modest box would saturate the CPU and the single
whisper-server and eventually wedge the recording itself.

**Why it's subtle.** It works fine in a demo (short clips) and only fails on long
recordings — exactly when a user cares. Two distinct failure modes are tangled:
unbounded per-tick work *and* scheduling ticks faster than the box can service
them. Fix one and you still ship the other.

**Solution.** Each tick is bounded to a rolling 15-second tail window
(`PREVIEW_WINDOW_SAMPLES`) so per-tick work is constant; a tick needs ~1 s of new
audio before it spends a transcription; and the cadence is adaptive —
`next_preview_interval` never schedules faster than the last tick actually took
(clamped to `[base, 8 s]`), so a slow box self-throttles instead of thrashing. The
authoritative full transcript is still produced from the complete file after stop.

`recorder/preview.rs:36-79` — `PREVIEW_WINDOW_SAMPLES`, `PREVIEW_MIN_NEW_SAMPLES`, `next_preview_interval`

### A forward-growing caption stitcher across a sliding window

**Problem.** Because the preview transcribes only the last ~15 s, once the window
slides each transcription begins partway through the speech. Naively *replacing*
the displayed text makes the caption's start jump around ("rewind"); naively
*appending* re-appends ~15 s of already-shown words (permanent duplication, since
the committed caption only grows).

**Why it's subtle.** Whisper re-tokenizes and revises the leading words of each
window between ticks, so a character-level or first-word overlap match silently
fails and the duplication is permanent. The freshest window is the source of truth
for the speech it covers, so you have to anchor on the *window*, not the committed
text, and tolerate a few revised leading words.

**Solution.** `stitch_preview` treats the shown text as a committed prefix and
appends only the genuinely-new tail, found via the longest *word-boundary* overlap
between the committed suffix and the window prefix. Words are normalized
(lowercased, punctuation stripped) before comparison, and the overlap may start a
few words into the window (`MAX_LEADING_SKIP`) so a revised leading word can't
defeat the match. It's a pure function, unit-tested with no audio round-trip.

`recorder/preview.rs:141-191` — `stitch_preview`

### An always-on-top overlay that doesn't freeze the app when dragged

**Problem.** The frameless, transparent, always-on-top live-caption overlay used a
`data-tauri-drag-region`, so dragging it entered Windows' modal move-loop — which,
on a transparent WebView2 window sharing the event loop, blocked the *entire app*,
often permanently.

**Why it's subtle.** The drag region is the idiomatic Tauri way to move a frameless
window, so the bug reads as correct code; the hang is specific to a transparent,
always-on-top WebView2 window on the shared loop. The fix also has to remember
position per window across runs and distinguish first-run placement from a restored
position across multi-monitor layouts.

**Solution.** The overlay drags via manual `setPosition` (coalesced to one move per
frame) and never enters the OS move-loop; `tauri-plugin-window-state` persists
geometry per label; and an off-screen sentinel cleanly separates first-run (place
bottom-center) from a restored position — instead of guessing from coordinate
signs, which mis-flags monitors left of or above the primary.

`overlay.rs:27-111` — position persistence, `OFFSCREEN_SENTINEL`, drag rationale

---

## Transcription-engine supervision

### Serializing the whisper-server so preview never starves the real transcript

**Problem.** Live preview and the final transcription both talk to one local
whisper-server. Under preview, long recordings and meetings hit
`Whisper timed out after 60s` because preview ticks contended with the
latency-critical final call.

**Why it's subtle.** It's invisible until load: one serial server, multiple async
callers, the final transcription is latency-critical while preview is best-effort.
The fix has to make preview *yield* without ever blocking the final call — and the
same permit must also serialize a mid-pipeline model-override restart.

**Solution.** A `whisper_sem` semaphore: the final transcription holds the permit
for its whole STT call, and preview only runs a tick when the permit is free. A
one-job model-override swap also happens under the permit, so nothing talks to the
server mid-restart.

`pipeline.rs:13-22` (the serialization invariant), `recorder/preview.rs:296-394` (preview yields the permit)

### Per-job model override without mutating global config

**Problem.** Re-transcribing one recording against a different local model means
restarting the shared bundled whisper-server onto that model — *for that one job
only* — while previews and other queued jobs keep running the configured model, and
the configured model is restored on every exit path.

**Why it's subtle.** The naive approach (temporarily writing the model into the
process-global config) races a concurrent `ReloadConfig` and can leak the forced
model onto an unrelated queued job, and it makes the server thrash and mass-fail
other jobs — the original #49 bug. You need exactly one restart-to-override and
exactly one restore, surviving cancels and errors.

**Solution.** `apply_model_override` publishes the override to the supervisor
(local) or clones it into a per-job config (cloud); `effective_model_path`
centralizes "what should the server run right now"; and a `Drop` guard
(`WhisperOverrideGuard`) clears the override on every pipeline exit path. The swap
runs under the `whisper_sem` permit and waits for `/health` before transcribing.

`pipeline.rs` — `WhisperOverrideGuard`, `apply_model_override`; `whisper_supervisor.rs` — `effective_model_path`

### Effective-port negotiation between two whisper-servers

**Problem.** The bundled whisper-server's preferred port (5809/5810) can be squatted
by another app. The daemon must fall back to a free OS-assigned port, publish the
live value, and ensure the main and preview servers never collide — including
mid-restart, when a model override re-runs the port probe and the server may come
back on a different port.

**Why it's subtle.** The configured port is only a *preference*, and a model-override
restart re-probes, so any consumer that caches a port (final transcription, preview,
dictation, the Doctor probe, the readiness check) will dial a dead one. Two siblings
probing concurrently can also race onto the same port.

**Solution.** A pre-flight probe routes around a squatter, excluding the sibling's
published and configured ports; the chosen port is published to
`AppState::whisper_ports` *before* spawn (so the sibling's probe sees it) and cleared
when down. Every consumer resolves effective-or-configured at provider-build time,
and the readiness wait re-evaluates the base URL through a closure on each poll.

`whisper_supervisor.rs` — effective ports, `port_is_free`; `pipeline.rs` — `wait_for_whisper_ready` closure

### Cancellable crash-backoff so a Doctor "Fix" is never lost

**Problem.** A whisper-server crash triggers exponential backoff (2 s → 60 s). A
user pressing Doctor's **Fix** while the supervisor slept out a 60 s backoff saw
nothing happen — the restart signal was lost and the respawn still waited the full
window.

**Why it's subtle.** `tokio::sync::Notify` stores no permit for `notify_waiters`, so
a notify fired while nobody is awaiting it simply vanishes; a plain
`tokio::time::sleep` is deaf to it. The non-obvious part: the only path that heals a
*hung* (not crashed) server is this very restart signal, so swallowing it during
backoff defeats the recovery feature.

**Solution.** `backoff_pause` `select!`s over the sleep, the restart notify, *and*
shutdown, so the pause is cancellable by either signal; a restart respawns
immediately and shutdown returns. The same pattern guards both the main and preview
supervisor loops.

`whisper_supervisor.rs:56-83` — `BackoffWake`, `backoff_pause`

---

## Meeting capture & track alignment

### Wall-clock alignment of a sparse loopback track against a dense mic

**Problem.** A meeting records two tracks — mic + system loopback — that must land on
one shared timeline. WASAPI loopback is *sparse* (it drops leading silence and hands
back only audible segments) while the mic is a *dense* continuous buffer.
Mis-aligning them desyncs the merged conversation.

**Why it's subtle.** You cannot treat both tracks the same: relocating a dense mic
track to its "first content" instant corrupts it, and failing to relocate a sparse
loopback track collapses internal silences. Telling them apart needs several
corroborating signals (a capture-window deficit, content at the buffer head, content
arriving late on the wall clock) because any single signal mis-classifies under CPU
load.

**Solution.** `align_one_track` classifies a track as sparse only when it is
non-dense **and** missing significant capture **and** has content at the buffer head
**and** first-content is late on the timeline; only then is it relocated to its
wall-clock first-content instant. The dense mic is always placed at recorder start.
Pure and regression-tested.

`meeting_align.rs:86-146` — `align_one_track`; `meeting_align.rs:10-43` — `TrackAlignInput.dense`, `AlignedTrack`

### Gap-filling on the audio clock, not the wall clock

**Problem.** To keep a meeting's system track continuous — so pausing a video
mid-meeting doesn't collapse the gap and desync the tracks — real silence is inserted
for wall-clock gaps. But sizing that silence as *wall-time-elapsed minus
samples-processed* read CPU stalls as fake gaps and stuffed extra silence in, running
the track long and desyncing the meeting.

**Why it's subtle.** The intuitive measure conflates two different causes of "missing"
samples: genuine device silence vs. the audio worker simply being late under load.
Only the former should be back-filled.

**Solution.** The fill is sized against what the *device actually delivered*, counted
at the capture callback, and is pause-aware — a meeting pause freezes both tracks with
no back-filled silence.

`meeting_align.rs:3-8` — `QUIET_THRESHOLD`, the sparse deficit; CHANGELOG (loopback gap-filling on the audio clock)

### Track-aware Meeting Mode: don't diarize the mic

**Problem.** A meeting's mic track is a single voice (yours), so running the diarizer
on it only burned time and produced spurious multi-speaker labels on one person. Only
the system/loopback track needs diarization.

**Why it's subtle.** The clean fix requires the pipeline to read the recording's
`track` + `meeting_id` *before* building the provider, thread a hint through the
transcription trait, and reuse the canonical `[Speaker N]` machinery (not invent a
`[You]` marker) so diarized-detection and the merged view keep working — while
ensuring cloud providers that ignore the hint don't get an orphaned "You" row.

**Solution.** A `DiarizationTrack` hint (`Diarize` / `FixedSpeaker("You")` / `Plain`)
is derived from the catalog row; the local `FixedSpeaker` branch skips speakrs
entirely and wraps segments under one `[Speaker 1]` turn via `label_all_as`, and a
`fixed_speaker_applied` flag gates the daemon's if-absent "You" `speaker_names` write
so a later rename survives a retranscribe.

`transcription.rs:87-133` — `DiarizationTrack`; `diarization.rs:241` — `label_all_as`

---

## Speaker diarization

### Word-level speaker attribution off the per-frame activation matrix

**Problem.** Whole-segment diarization mislabels a word that straddles a speaker
hand-off, because a Whisper segment can span a turn boundary. Attribution has to be
*per word*: map each word's time span onto the diarizer's per-frame activation matrix
and pick the speaker who owns most of its frames.

**Why it's subtle.** The frame grid is ~17 ms and uses speakrs' real
`frame_step`/`frame_duration` constants (0.016875 / 0.0619375) — getting these wrong
inflates every turn ~59× and scrambles attribution. Word- and segment-level numbering
must also agree (the same `SPEAKER_{k:02}` → stable 1-based index), and you need a
clean fallback for cloud / segments-only transcripts.

**Solution.** `assign_words` maps each word's `[start, end]` to a frame range via
`frame_for_time`, sums activations per speaker column (made exclusive so each frame has
one winner) and argmaxes via `dominant_column`; columns map to the same stable indices
`label_segments` uses. A graceful fallback to segment-level attribution reproduces the
old labels exactly when word timings are absent.

`diarization.rs:372` — `assign_words`; `diarization.rs:289` — `frame_for_time`, `column_label`, `dominant_column`

### Island-smoothing to kill mid-sentence speaker flips

**Problem.** Per-word argmax over short, noisy frame windows chops a single continuous
voice into phantom turns ("the fact that women / `[Speaker 2]` going to do what they /
`[Speaker 1]` want"), which no real turn-taking does. Wall-clock-only smoothing
(sub-0.6 s flickers) missed multi-word noise islands.

**Why it's subtle.** You must remove noise islands *without* merging genuine short
interjections ("Yes." between another speaker's sentences). The distinguishing signal
is structural, not durational: a short run bracketed by the *same* speaker on both
sides is almost always noise, while a run at a real transition (different speakers
either side) is a real turn. And whisper.cpp emits subword tokens, so the token-count
ceiling is ~2× the spoken-word count.

**Solution.** `smooth_word_speaker_runs` absorbs a lone word always; a
same-speaker-bracketed run up to `MAX_ISLAND_WORDS` (10 tokens ≈ 5 words); or a larger
run strictly shorter than both neighbours under a ~24-token ceiling — into the
surrounding speaker. Per-word attribution is otherwise kept, so a genuine hand-off
inside a segment still splits.

`diarization.rs:509-520` — thresholds + rationale; `diarization.rs:597` — `smooth_word_speaker_runs`

### Voiceprint centroid merge to fix a wrong speaker count

**Problem.** speakrs' clustering (AHC seed → VBx) sometimes over-splits *one* real
voice into two clusters, so a 2-person recording reports 3 speakers and flip-flops
between the phantom pair. Timing smoothing can't fix this — it's a count error, not a
timing error.

**Why it's subtle.** There's no threshold inside the diarizer that separates an
over-split voice from two genuinely different speakers — until you measure the
embeddings. Two fragments of one voice score far higher against each other (~0.57
cosine) than two distinct voices do (~0.33–0.46), but that calibration only emerges
from real recordings — and you must fold the merged cluster's activations into a
canonical column so *both* word- and segment-level attribution see the corrected set.

**Solution.** After diarization, `cluster_centroids` computes an L2-normalized
voiceprint per cluster from per-(chunk, speaker) embeddings; `merge_similar_clusters`
single-linkage-merges any pair with cosine ≥ 0.50 (`SPEAKER_MERGE_COSINE`) via
union-find, keeping the smallest column index canonical. Activations and segment spans
are relabelled accordingly.

`diarization.rs:1202` — `SPEAKER_MERGE_COSINE`, `cluster_centroids`, `merge_similar_clusters`

### A named voice's cached centroid: robust *and* duration-weighted

**Problem.** A named voice in the Speaker Library is one cached centroid recomputed
from every capture enrolled under it. Two things spoil a naive mean of those
captures: a single mis-named wrong-speaker capture drags the template off the real
voice, and a one-word capture (a quick "yeah") gets the same vote as a clean
five-minute one — so the template drifts toward whoever happened to be captured
often, not whoever was actually heard most.

**Solution (two layers, in order).** `recompute_named_centroid` pulls every linked
capture with its `duration_ms`, then:

1. **Outlier rejection (geometry only).** With ≥ 4 captures, `drop_centroid_outliers`
   takes a provisional *unweighted* mean and drops any capture below a hard cosine
   floor (`0.2`) or `mean − 2·stddev`. Duration is deliberately *not* used here —
   a long sample of the wrong speaker is still the wrong speaker, so length must
   never buy an outlier its way in.
2. **Duration-weighted mean (survivors only).** `voiceprint::weighted_mean_centroid`
   then averages the *surviving* captures weighted by their speaking duration, so a
   long clean sample outvotes a brief one. Weights are ratios only (ms in practice).

**Backward compatibility.** A capture's `duration_ms` defaults to `0` (migration
`20260620000000`), and the weighted mean treats a non-positive/non-finite weight as
weight 1 — i.e. equal weighting. So a library built before this feature (all zeros)
recomputes to the *exact* old unweighted centroid until new, duration-bearing
captures arrive. The capture-time duration is the sum of that speaker's segment
spans, summed in the pipeline from the persisted segment timeline.

`catalog/speakers.rs` — `save_speaker_voiceprint` (stores `duration_ms`), `drop_centroid_outliers`,
`recompute_named_centroid`; `voiceprint.rs` — `weighted_mean_centroid`;
`pipeline.rs` — per-speaker duration sum at capture.

### A centroid is only comparable within its own embedding model

**Problem.** Named-speaker recognition matches a captured voiceprint against the
library by cosine similarity. But a centroid is only meaningful relative to the
embedding model that produced it: swap `[diarization].models_dir` for a different
ONNX bundle and the new vectors live in a different space, so a cosine against an
old centroid is noise — yet it can still clear the match threshold *by luck* and
silently bind the wrong named voice.

**Why it's subtle.** The score itself looks healthy; nothing in the cosine reveals
that the two vectors came from incompatible models. And the fix can't just hard-fail
old data — a library enrolled before model tracking existed has no model tag, and
must keep matching exactly as it did before so an upgrade doesn't erase recognition.

**Solution.** Each captured row stores the `embedding_model` it was produced under
(`speaker_voiceprints.embedding_model`, from `diarization::embedding_model_id`). The
match query excludes centroids from a *different* model — `sv.embedding_model = ?
OR sv.embedding_model = ''` — so only same-model (or untagged-legacy) centroids are
ever compared. The empty string is the wildcard: pre-migration rows default to `''`
("model unknown") and keep matching everything, so an existing library is unchanged
until new, model-tagged captures arrive.

`catalog/speakers.rs` — `save_speaker_voiceprint` (`embedding_model`), `named_voice_centroids` (the same-model filter); `diarization.rs` — `embedding_model_id`; migration `20260623000002_voiceprint_model_version.sql`

### Keeping written words atomic across a hand-off, and subword spacing

**Problem.** Per-word argmax places boundaries on a ~17 ms grid, so a token at a
hand-off can land on the wrong side: a `.` stranded onto the next speaker, or `That's`
split as `That`[A] / `'s`[B]. Separately, rejoining Whisper's subword/punctuation
tokens with a single space produced `I don 't know`, `over ste pped`, and a space
before every `.`.

**Why it's subtle.** Whisper emits subword and punctuation tokens whose word
boundaries live in a leading-space marker it strips; a naive trim-and-space-join
destroys that. The same marker also fixes attribution: a non-word-start token must
inherit the speaker of the word-start it attaches to, so a single written word is
never divided between speakers and a turn never opens with orphaned punctuation.

**Solution.** Capture Whisper's leading-space marker per token
(`WordSpan.leading_space`), rejoin by it (so `over`+`ste`+`pped` fuse and `weapon?`
attaches cleanly), and in `coalesce_subword_tokens` force every non-word-start token to
inherit its host word's speaker. The marker is persisted (a migration) and sent over
IPC so the Synced view honours it too.

`diarization.rs:75-90` — `WordSpan.leading_space`; `diarization.rs:755` — `coalesce_subword_tokens`

---

## The processing pipeline & Playbook

### A Strategy-B recipe executor with byte-identical re-runs and a config migration

**Problem.** Cleanup / title / summary / tags / hooks were scattered across
`[llm_post_process]` / `[title]` / `[summary]` / `[auto_tag]` / `[hook]` toggles, and
on-demand re-runs duplicated pipeline logic, so the two could drift. The Playbook had
to become the single source of truth *without changing behaviour byte-for-byte* for
existing setups.

**Why it's subtle.** The executor gates each step on recipe *membership* (not legacy
flags), but the legacy Re-run paths layer one-time overrides that have to be mirrored
into the matching Playbook entries on a per-job *clone* — otherwise a custom
model/prompt is silently ignored. A one-time migration must fold live config into
entries exactly once, persist a latch, and self-heal if the first save failed, all
while keeping API keys encrypted.

**Solution.** The executor is a thin dispatcher over the proven streaming/persistence
primitives, so parity is automatic. `apply_rerun_overrides` mutates only a config
clone (avoiding the `ReloadConfig` race that leaked a forced pipeline onto other jobs)
and mirrors overrides into entries; `ensure_default_recipe_steps` slots forced-on
steps back into canonical order; and a single `schema_version` integer (each
migration owns a version step) makes the migrations idempotent — superseding the
old per-feature `playbook_migrated` / `hooks_migrated` latches, whose values are
still read once on load to infer the starting version of a pre-versioning config.

`pipeline.rs` — `apply_rerun_overrides`, `ensure_default_recipe_steps`, `resolve_recipe`, `run_transform_steps` (the recipe executor)

### A Re-run that overrides the recipe the user chose, not always the default

**Problem.** The Re-run modal lets you re-run a recording through a *named* recipe
(its "Run through" picker) while layering one-time per-step model/prompt overrides
on top. Under the executor those overrides have to be mirrored into the matching
Playbook entries, and the "skip cleanup" opt-out has to drop the cleanup step —
but both were always applied to the **default** recipe, so picking a non-default
recipe ran it unmodified and the overrides silently went nowhere.

**Why it's subtle.** The mirror still *worked* for the common case (the default
recipe), so the bug only surfaced when a user both chose a different recipe *and*
overrode a step — and even then it failed quietly: the run completed, just with the
recipe's own models instead of the chosen ones. The fix also has to resolve the
target safely: an unknown or deleted recipe id must fall back to the default rather
than mutate nothing (or panic), and every mutation must stay on the per-job clone so
the persisted recipe is never touched.

**Solution.** `apply_rerun_overrides` first resolves a `target_recipe` — the chosen
id if it exists in `cfg.recipes`, else `DEFAULT_RECIPE_ID` — and applies both the
cleanup-step drop (post-process opt-out) and the forced-on steps to *that* recipe on
the clone. The frontend matches the model to one scope-first switch: the Re-run /
Models modal's first control is "Just this run" vs "My defaults", and the footer
shows exactly one scope-bound primary button ("↻ Run once" or "💾 Save defaults"),
so a one-time override and a config write can never be confused for each other.

`pipeline.rs` — `apply_rerun_overrides` (the `target_recipe` resolution); `frontend/src/components/ModelPicker.ts` — the scope switch + single primary action

### Robustly parsing tags and titles out of chatty LLMs

**Problem.** Local and cloud LLMs wrap their answer in prose, code fences, and
announcements (`Title: …`, `[1] as cited…`, `[hope that helps]`). A greedy
first-`[` … last-`]` slice spanned the surrounding prose, failed to parse, and
comma-split the whole reply into junk tag candidates.

**Why it's subtle.** The output is adversarially messy in ways that only show up across
many models, and the naive "find the JSON" slice fails exactly when there are extra
brackets in the prose. You also have to canonicalize against the existing vocabulary
case-insensitively so `Code` doesn't mint a duplicate of `code`, and cap/de-dup without
losing real tags.

**Solution.** `parse_tag_names` scans *every* `[` and uses a streaming deserializer to
parse exactly one well-formed string array, ignoring trailing prose; a non-string array
fails fast and the scan moves on. Names are trimmed of quotes/hashes/bullets,
length-capped, de-duped, canonicalized against existing tags, and optionally
auto-accepted. `sanitize_llm_title` strips `Title:` prefixes and wrapping
quotes/markdown and caps at 8 words.

`pipeline.rs` — `parse_tag_names`, `sanitize_llm_title`

### Idle-based streaming timeout for slow local LLMs

**Problem.** Cleanup/summary/title applied `timeout_secs` (default 30 s) as a *total*
deadline including a streaming Ollama response, so a healthy-but-slow generation on a
CPU box (or a cold model load under memory pressure) was aborted mid-stream, surfacing
as the opaque `Ollama stream error: error decoding response body`.

**Why it's subtle.** A total deadline conflates "the model is stuck" with "the model is
slow but producing tokens" — the right signal is *idle time between chunks*, not
wall-clock total, and the first-token cold load needs its own floor.

**Solution.** The streaming path bounds idle time between chunks (floored to ≥120 s,
covering the cold first-token load) and lets total generation run as long as tokens keep
arriving; the non-streaming call gets the same floor. A genuine stall fails with an
actionable message pointing at a smaller model or a higher timeout.

`pipeline.rs` — `run_llm_stage` streaming, coalesced deltas

---

## In-place dictation

### Type-first dictation: insert the text exactly once across two paths

**Problem.** With `full_pipeline` + `type_first`, the recorder types the quick
transcription the instant it's ready while the full pipeline runs everything else in the
background. The pipeline must *not* type again or the text lands twice — but a
recipe-routed in-place hotkey must type the recipe's *result*, not the raw quick pass.

**Why it's subtle.** Two independent code paths (the type-first task and the pipeline's
end-of-run typing) could each insert, and the correct one depends on a three-way
condition (`full_pipeline`, `type_first`, `recipe_routed`). Get the mirror wrong and you
either double-type or never type. The decision must be pure so it's testable without an
input simulator.

**Solution.** `pipeline_should_type` encodes it: a type-first pass ran iff
`full_pipeline && type_first && !recipe_routed`, and the pipeline types iff one did
*not*. `spawn_type_first` owns none of the catalog state (the queued pipeline does), so
there's exactly one authoritative insertion.

`pipeline.rs` — `pipeline_should_type`; `in_place.rs` — `spawn_type_first`

### A dictation fast lane that never waits behind the queue or a meeting

**Problem.** In-place dictation needs Wispr-Flow-class latency: transcribe-and-paste must
not wait behind a meeting that's mid-transcription on the single serial whisper permit,
must not run diarization, and must not pay an LLM round-trip unless asked. An earlier bug
ran the live-preview loop during dictation, so its per-second ticks contended for the
permit and the paste was delayed or never happened.

**Why it's subtle.** The naive gate was on `streaming_preview`, not the dictation flag,
so dictation silently inherited the preview loop; and `stop()` waited out an in-flight
preview tick before the dictation could transcribe.

**Solution.** The fast lane skips the inbox queue and the full pipeline — transcribe with
the dictation provider → rule-based polish (zero latency) → type/paste → persist in the
background — and in-place dictation skips live preview entirely so the whisper permit is
free for the latency-critical transcribe.

`in_place.rs:1-19` — fast-lane contract; `recorder/mod.rs:8-9` (in-place branches off the queue), `recorder/preview.rs:589-619` (dictation skips live preview)

### Clipboard-preserving paste delivery

**Problem.** Pasting dictated text via the clipboard (set → Ctrl+V → restore) is
near-instant for long text, but it would clobber whatever the user had on the clipboard,
and the restore can fire before the target app has consumed the paste.

**Why it's subtle.** Ctrl+V is processed asynchronously by the receiving window, so an
immediate restore races the paste and the wrong text lands. The previous clipboard
contents may also be a non-text format (an image the user was about to paste), which a
text-only backup silently drops. Blocking input APIs also can't run on the async runtime.

**Solution.** `paste_blocking` backs up text (falling back to an image), writes the
transcript, sends Ctrl+V, sleeps 150 ms so the app consumes it, then restores the
previous text or image. `type_blocking` is the fallback for apps that block paste, and
both run on `spawn_blocking`.

`in_place.rs:474-600` — `type_at_cursor`, `paste_blocking`, `type_blocking`

---

## Search & recall

### Chunked hybrid search: chunking + RRF fusion + cosine calibration

**Problem.** Embedding a whole transcript into one mean-pooled vector silently drops
everything past the model's 256-token limit and smears many ideas into one averaged
vector, so paraphrase recall on longer notes is poor. Vector search also misses exact
terms (proper nouns, code identifiers) that lexical FTS5 nails — and vice versa.

**Why it's subtle.** You can't simply pick one retriever, and you can't naively combine
their scores — cosine ∈ ~[0,1] vs. BM25, which is unbounded and sign-flipped. A single
hard cosine floor silently drops genuine paraphrase hits sitting just under it. And raw
cosine isn't a percentage (a strong match for all-MiniLM lands ~0.55–0.75, unrelated
text ~0.1), so showing it directly misleads users.

**Solution.** `chunk_transcript` splits into overlapping, sentence-aware ~80-word chunks
(under the token limit, with a one-sentence overlap so a straddling idea is wholly
contained somewhere); a recording is scored by its best chunk (max-sim).
`reciprocal_rank_fusion` combines the vector and FTS5 rankings scale-free
(`1/(k+rank)`, `k=60`). `calibrate_cosine` maps the model's useful band `[0.15, 0.70]`
onto 0–100% for the relevance chip.

`chunk.rs:1-50,105` — chunking, `chunk_transcript`; `fusion.rs:54-112` — `reciprocal_rank_fusion`, `calibrate_cosine`, `COSINE_FLOOR/CEIL`

---

## Security & the trust boundary

### An SSRF guard tuned for a local-first product

**Problem.** Outbound webhooks must *not* be blockable in the common case — a webhook
into *this* machine (local n8n / Home Assistant) is the feature's primary job — yet must
not let a mistyped or hostile URL bounce transcripts at an internal LAN service or the
cloud metadata endpoint.

**Why it's subtle.** A blanket private-range block breaks the local-first use case, so a
three-tier policy is needed. The bypasses are the hard part: a DNS name can resolve to a
private address (resolve-and-classify every address, most restrictive wins),
IPv4-mapped IPv6 (`::ffff:a.b.c.d`), the NAT64 well-known prefix (`64:ff9b::/96` embeds
an IPv4 a gateway translates), and CGNAT space (`100.64/10`) all smuggle internal hosts
past a naive check. Redirects can also re-point a vetted URL.

**Solution.** `HostClass` classifies loopback (any scheme) / private (opt-in) / public
(HTTPS-only) and validates *before any byte leaves*: `classify_v4` treats CGNAT as
private, `classify_ip` decodes IPv4-mapped and NAT64-embedded addresses to their inner
v4, `classify_resolved` takes the most restrictive class across all resolved addresses,
and redirects are never followed. HMAC-SHA256 signing and reserved-header protection
guard the body.

`webhook.rs:54-130` — `HostClass`, `classify_v4`, `classify_ip`, `classify_resolved`; `webhook.rs:26-52` — signing, `RESERVED_HEADERS`

### At-rest API-key encryption with zero-touch migration and cross-machine safety

**Problem.** API keys were written to `config.toml` in plaintext. They must be encrypted
per-user at rest, migrate existing plaintext keys with no manual step, stay testable off
Windows (CI), and degrade safely if a config is copied to another user or machine.

**Why it's subtle.** The scheme must be backwards- *and* forwards-compatible at once: a
legacy plaintext value must read verbatim and re-encrypt on next save; an empty key must
stay empty so "is a key set?" checks survive; an undecryptable `dpapi:` blob (wrong
user/machine, or non-Windows) must read as *unset* rather than feeding a garbage key to a
provider. It also composes with the separate WebView-masking layer.

**Solution.** `secret_crypto` wraps Windows DPAPI (`CryptProtectData`) with a versioned
`dpapi:v1:` prefix; `protect`/`unprotect` handle empty, plaintext-migration, and
decrypt-failure-as-unset; off Windows it's a passthrough so tests round-trip. Masking
sees the encrypted value and replaces it with a sentinel before crossing to the renderer.

`secret_crypto.rs:1-70` — `protect`, `unprotect`, `PREFIX`; `secret_crypto.rs:91-120` — `dpapi_protect` FFI

### A hook subprocess that can't deadlock or leak a runaway process

**Problem.** User-configured hooks are arbitrary subprocesses fed the recording's payload
as JSON on stdin. A chatty hook can write more than the OS pipe buffer and deadlock; a
runaway hook must be killable on timeout; and subprocess output can carry
credential-shaped secrets back across the IPC trust boundary to the GUI.

**Why it's subtle.** `wait_with_output` consumes the child handle, leaving no way to kill
a timed-out process — and on Windows, Tokio's `Drop` doesn't terminate the process, so a
naive timeout *leaks* the child. Draining must run concurrently with the wait, but keeping
the child handle *out* of the drain future is what preserves the ability to kill it.

**Solution.** `HookRunner::run` drains stdout/stderr concurrently with `child.wait()`
while keeping the child handle outside the drain future so a timeout can explicitly kill
it; `CREATE_NO_WINDOW` avoids a console flash; and `redact_secrets` scrubs
credential-shaped tokens (`sk-`/`ghp_`/`AKIA`/`Bearer`/`key=`) and caps length before
output crosses back. `RefireHook` over IPC is allowlist-gated.

`hook.rs:1-120` — `HookRunner::run`, concurrent drain, kill-on-timeout

---

## IPC & desktop reliability

### A bounded NDJSON codec resilient to flooding and partial frames

**Problem.** The daemon ↔ client protocol is newline-delimited JSON over a named pipe. A
malicious or buggy peer that never sends a newline would grow the decode buffer without
limit and OOM the daemon; CRLF blank lines and multi-frame reads must not stall a
request/response.

**Why it's subtle.** The bugs hide in the framing edges: returning `Ok(None)` on a blank
line leaves an already-buffered frame after it unparsed until the next read (stalling a
same-buffer request/response); a CRLF blank line leaves a lone `\r` that parses as invalid
JSON and fuses the stream; and the size cap must error only on an *unterminated*
over-cap frame, not a legitimately large one mid-arrival.

**Solution.** `JsonLineCodec` loops over complete lines (skipping empty/CRLF-trimmed ones
so a same-buffer frame is parsed immediately) and errors once buffered unterminated data
exceeds an 8 MiB cap (`MAX_FRAME_BYTES`). It's paired with lenient
`ServerRequest::Unknown` decoding, so an unknown request from a newer client returns an
error `Response` instead of tearing down the pipe.

`codec.rs:12-104` — `MAX_FRAME_BYTES`, decoder loop, tests; `schema.rs:1-13` — lenient decoding

### A bridge reconnect that won't double-execute mutations or spawn-storm

**Problem.** The tray's single request pipe should self-heal across daemon restarts (a
Doctor **Fix** shouldn't require closing the window), but transparent
reconnect-and-retry on any error can duplicate a non-idempotent mutation, and a burst of
UI clicks during an outage can spawn-storm the daemon.

**Why it's subtle.** A transport error genuinely can't distinguish "the daemon never saw
the request" from "the daemon ran it but the reply was lost when the pipe dropped" —
silently re-sending in the latter case double-executes (e.g. `ImportRecording` mints a
fresh `RecordingId` per call). And a lazily-reconnecting holder must back off without
ever permanently giving up, since the daemon may start long after the tray.

**Solution.** A `BridgeSlot` retries the *connect* with bounded exponential backoff
(250 ms → 10 s cap, reset on success) but never blindly retries the *request*;
`is_retry_safe` gates automatic resend to idempotent requests only, so a mutation that
might have run is surfaced rather than silently repeated. The event stream re-attaches in
the background the moment the daemon reappears.

`bridge.rs` — `BridgeSlot`, `is_retry_safe`, reconnect backoff

---

## Data integrity & lifecycle

### WAV atomic write and idempotent crash recovery

**Problem.** A crash mid-write could leave a corrupt WAV at the final path; and a daemon
crash in the window between writing `done/<id>.json` and removing `processing/<id>.json`
could re-queue an already-completed item on the next startup.

**Why it's subtle.** Both are narrow timing windows that never show in normal operation,
and Windows rename semantics differ from POSIX. Crash recovery must also sweep every
in-flight status (recording / transcribing / queued / paused) to a terminal state without
resurrecting completed work — the `done`+`processing` pair is the tell that the item is
actually finished.

**Solution.** Recordings write to a `.tmp` sibling and rename into place (handling Windows
rename correctly), so a crash never leaves a corrupt WAV at the final path. Startup
recovery detects the `done`+`processing` pair and drops the stale processing file instead
of re-queuing; orphaned `queued`/`paused` rows with no inbox file are swept to
`transcribe_failed`.

`pipeline.rs` — `finalize_canceled`; CHANGELOG (WAV atomic write, idempotent crash recovery)

### Full kill-on-close process tree via Windows Job Objects

**Problem.** The daemon spawns several children (whisper main + preview + optional
dictation servers, an owned Ollama). On an unclean death (panic, Task Manager "End task")
these would orphan, squatting ports and holding models resident.

**Why it's subtle.** Graceful shutdown handles the clean case; the hard case is the unclean
one — and you must *not* kill an Ollama the user was already running. Ownership has to be a
sticky ledger decided at first probe, and the tray itself must optionally hold the daemon in
a job (decided at spawn time) so even End-task reaps the whole tree.

**Solution.** The daemon holds a kill-on-close Job Object that every spawned child joins
(`assign_to_daemon_job`, best-effort); `sweep_stray_servers` also kills the whisper-server
processes it identifies by their transcription command line (the `/v1/audio/transcriptions`
marker every spawned server carries) before a respawn, so an unrelated `whisper.cpp` is spared.
An ownership ledger marks an already-running
Ollama `NotOurs` forever (never killed); only a daemon-launched one is `Owned` and reaped,
with single-flight spawn so concurrent steps can't double-spawn.

`whisper_supervisor.rs:34-41,89-103` — job membership, `assign_to_daemon_job`

---

## See also

- [Architecture](architecture.md) — the process model and data flow these pieces live in.
- [Backend internals](internals.md) — the catalog, search, and pipeline at the code level.
- [Threat model](threat_model.md) — the security boundary the guards above defend.
- [Configuration reference](config_reference.md) — every knob the behaviours above expose.
