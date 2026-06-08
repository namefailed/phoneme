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
  false-positive story. Depends on the v1.10 embedding/chunking work existing first.
- **Promote when:** chunked embeddings land and a user actually complains about dupes.

### Chapter markers
Auto-split a 90-minute meeting on long silences into navigable chapters.
- **Why parked:** silence detection already exists, but it's a fair bit of UI +
  export work for a need nobody's voiced yet.
- **Promote when:** someone records genuinely long sessions and wants navigation.

### Live meeting subtitles overlay
Floating captions during Meeting Mode without alt-tabbing.
- **Why parked:** an OS-level always-on-top overlay synced to the (sparse) system
  track is real work, and the bounded live preview already covers "am I being
  captured?" Performance + multi-monitor edge cases are a tax.
- **Promote when:** there's demand for true real-time captioning, not just a preview.

### Team glossary sync
Shared names/terms (custom vocabulary) across machines/teammates.
- **Why parked:** depends on a sync story, which is a different shape from the
  opt-in S3 cloud-sync planned for v2.0. Single-machine custom vocabulary (v1.10)
  has to exist and prove useful first.
- **Promote when:** custom vocabulary ships and teams ask to share it.

---

## 🛑 Deferred with prejudice — likely "no," kept for the record

These were considered and pushed off the roadmap. They're a hair short of
"Explicitly Not Doing" — listed here so we don't re-litigate them every quarter.

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

---

*See also: [`ROADMAP.md`](../ROADMAP.md) → "Explicitly Not Doing" for ideas we've
firmly rejected (favorites, duration filter, backup ZIP, Azure/AWS STT, etc.).*
