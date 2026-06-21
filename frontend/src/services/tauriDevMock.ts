/// <reference types="vite/client" />
/**
 * Dev-only Tauri IPC mock — lets phoneme render in a plain browser (a preview
 * pane, or `vite` opened directly), where `window.__TAURI_INTERNALS__`
 * doesn't exist and every `invoke()` would otherwise throw "Cannot read
 * properties of undefined (reading 'invoke')". It feeds canned recordings / tags
 * / config so the list, sidebar, and detail pane populate and the roving keyboard
 * cursor + glow animations can be exercised and screenshotted without the native
 * window.
 *
 * Safety — never affects the real app:
 *   - Installed only when this is a Vite dev build (`import.meta.env.DEV`) and
 *     there is no real Tauri runtime. In `cargo tauri dev` and production builds
 *     `window.__TAURI_INTERNALS__` is injected by Tauri, so the mock is skipped.
 *   - In a production build `import.meta.env.DEV` is statically false, so the whole
 *     block (and the `@tauri-apps/api/mocks` import) is dead-code-eliminated.
 *
 * It mocks only the commands the UI calls on mount / common interactions; events
 * are accepted (so `listen()` resolves) but never emitted.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { MASKED_SECRET } from "./llmModels";

/** A catalog tag (mirrors ipc.ts `Tag`); `color` is `#rrggbb` or null (accent). */
type Tag = { id: number; name: string; color: string | null };

// Mutable in the mock so add / rename / recolor / delete and attach / detach
// actually stick — the tag surfaces (detail-pane chips, Tag Manager, sidebar)
// can be driven end-to-end in the browser preview, not just rendered read-only.
const TAGS: Tag[] = [
  { id: 1, name: "work", color: "#cba6f7" },
  { id: 2, name: "personal", color: "#89b4fa" },
  { id: 3, name: "ideas", color: "#a6e3a1" },
  { id: 4, name: "todo", color: "#f9e2af" },
];

/** ISO timestamp `daysAgo` days back at HH:MM, so the list's Today/Yesterday/
 *  Last-7-days grouping renders naturally. Runs in the browser — Date is fine. */
function iso(daysAgo: number, h: number, m: number): string {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  d.setHours(h, m, 0, 0);
  return d.toISOString();
}

function rec(
  id: string,
  daysAgo: number,
  h: number,
  m: number,
  durMs: number,
  title: string,
  tagIds: number[],
  favorite: boolean,
  transcript: string,
  // Meeting tracks add `meeting_id` / `track` / `meeting_name` (and override
  // `diarized`) through this; standalone recordings leave it empty.
  extra: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    id,
    started_at: iso(daysAgo, h, m),
    duration_ms: durMs,
    audio_path: `/sample/audio/${id}.wav`,
    transcript,
    notes: "",
    model: "ggml-large-v3-turbo",
    cleanup_model: "gemma3:4b",
    status: "done",
    title,
    title_is_auto: true,
    favorite,
    pinned: false,
    diarized: id === "r11",
    user_edited: false,
    tags: tagIds.map((t) => TAGS.find((x) => x.id === t)).filter(Boolean),
    speaker_names: [],
    tag_suggestions: [],
    meeting_id: null,
    track: null,
    meeting_name: null,
    in_place: false,
    ...extra,
  };
}

// Fully synthetic placeholder data — no real content. Only here to render the UI
// in a browser preview; never shipped (see the import.meta.env.DEV guard).
const P1 =
  "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.";
const P2 =
  "Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";
const PARA = `${P1}\n\n${P2}`;
const CONVERSATION = [
  "[Speaker 1]: Hello, how are you today? I wanted to walk through the agenda before we get started.",
  "[Speaker 2]: Doing well, thanks. That sounds good — I think we should cover the timeline first.",
  "[Speaker 1]: Agreed. The first milestone is on track, but the second one might slip by a few days.",
  "[Speaker 2]: That's fine. Let's note the risk and move on to the open questions for now.",
].join("\n\n");

/* ── Synthetic meetings ────────────────────────────────────────────────────
   A meeting is 2+ recordings sharing a non-null `meeting_id`, one per captured
   track ("mic" = your voice, "system" = everyone on the call). The list folds
   the tracks into a single expandable group (groupRecordings), and the merged
   view interleaves them chronologically from each track's stored segments
   (mergeChronological) — the UI this fake data exists to exercise.

   Each segment is `{ start_ms, end_ms, text, speaker? }`, offsets into that
   track's audio; the tracks share a wall clock, so equal offsets are "the same
   moment". A system track can carry diarized `[Speaker N]:` turns (speaker set
   on its segments); a mic track is a single voice (speaker null). Both tracks
   need segments, or the merge falls back to the coarse by-source order. */
type Seg = { start_ms: number; end_ms: number; text: string; speaker: string | null };
const seg = (start_ms: number, end_ms: number, text: string, speaker: string | null = null): Seg =>
  ({ start_ms, end_ms, text, speaker });

// Meeting 1 — "Product sync call": your mic + a 2-person call on system audio.
const M1_MIC: Seg[] = [
  seg(0, 4200, "Hey everyone, thanks for hopping on. Can you both hear me okay?"),
  seg(13800, 18200, "Let's do it. The first milestone is basically done — we shipped the preview pane this week."),
  seg(24300, 29500, "It could slip a couple of days. The merged-view work is bigger than we scoped."),
  seg(35300, 39000, "Sounds good. I'll write that up right after the call."),
];
const M1_SYS: Seg[] = [
  seg(4500, 8800, "Yep, loud and clear on our end.", "1"),
  seg(9000, 13500, "Same here. Should we start with the roadmap?", "2"),
  seg(18500, 24000, "Nice. How's the second milestone looking? I heard it might slip.", "1"),
  seg(29800, 35000, "That's fine on our side. Let's just flag the risk in the notes and move on.", "2"),
];

// Meeting 2 — "Design critique": your mic + a teammate on system audio (single
// voice each, no diarization), a simpler two-source interleave.
const M2_MIC: Seg[] = [
  seg(0, 3800, "Alright, go ahead and share — I can see it."),
  seg(10300, 15000, "I like it. The Live Preview section finally lines up with the rest of the panel."),
  seg(21300, 25500, "Good call. I'll add those to the list and the detail view this afternoon."),
];
const M2_SYS: Seg[] = [
  seg(4000, 10000, "So this is the redesigned settings panel. The help text reads a lot more consistently now."),
  seg(15300, 21000, "Right. The only thing left is a back-to-top button on the long scrolling pages."),
];

// Meeting 3 — "Quarterly planning" — placed further down the list (older), a
// 2-person call diarized on the system track, to test the merged view + grouping
// deeper in a long, scrollable list.
const M3_MIC: Seg[] = [
  seg(0, 5200, "Thanks for making time. I want to lock the top three priorities for next quarter."),
  seg(16000, 22000, "Agreed. Let's put recall quality first, then the meeting views, then polish."),
  seg(33000, 39500, "Perfect. I'll write these up and circulate before Friday."),
];
const M3_SYS: Seg[] = [
  seg(5500, 11000, "Sounds good. My vote is we keep search quality at the very top.", "1"),
  seg(11200, 15500, "Same — and the merged meeting view needs a real polish pass.", "2"),
  seg(22500, 32500, "Works for me. Can we also reserve a little time for the smaller UI papercuts?", "1"),
];
// More meetings, scattered deeper in the list (days 4 / 8 / 14) so the merged +
// grouped meeting UI shows up at several scroll depths, not just near the top.
const M4_MIC: Seg[] = [
  seg(0, 4500, "Quick sync on the onboarding flow — where did we land on the wizard copy?"),
  seg(12000, 17000, "Got it. I'll tighten the review step and ship it behind the flag."),
];
const M4_SYS: Seg[] = [
  seg(5000, 11500, "We simplified it to three steps; the copy still needs a pass.", "1"),
];
const M5_MIC: Seg[] = [
  seg(0, 5000, "Retro time. What went well and what slowed us down this cycle?"),
  seg(20000, 26000, "Fair. Let's add a buffer for the diarization rework next time."),
];
const M5_SYS: Seg[] = [
  seg(5500, 12000, "Shipping the live preview felt great. The model thrash bug cost us a day.", "1"),
  seg(12200, 19000, "Agreed — and reviews piled up mid-week, that's the real slowdown.", "2"),
];
const M6_MIC: Seg[] = [
  seg(0, 4800, "Client check-in — they want the export formats and the saved searches."),
  seg(16000, 22000, "Perfect. I'll demo both on Thursday and send notes after."),
];
const M6_SYS: Seg[] = [
  seg(5200, 11000, "Both are in already; we just need to walk them through it.", "1"),
];

/** Build a track transcript from its segments: `[Speaker N]:` turns when the
 *  segments are diarized (matches the merged view's marker parsing), else plain
 *  prose joined by blank lines. */
function trackTranscript(segs: Seg[]): string {
  return segs
    .map((s) => (s.speaker != null ? `[Speaker ${s.speaker}]: ${s.text}` : s.text))
    .join("\n\n");
}

/** Map of track id → its stored segment timeline (drives get_segments). */
const SEGMENTS: Record<string, Seg[]> = {
  m1a: M1_MIC, m1b: M1_SYS,
  m2a: M2_MIC, m2b: M2_SYS,
  m3a: M3_MIC, m3b: M3_SYS,
  m4a: M4_MIC, m4b: M4_SYS,
  m5a: M5_MIC, m5b: M5_SYS,
  m6a: M6_MIC, m6b: M6_SYS,
};

/** One mock auto-chapter (drives get_chapters / suggest_chapters). */
type Chap = { start_ms: number; end_ms: number; title: string; summary?: string | null };

/** Map of track id → its stored chapters, derived from the segment starts so the
 *  dev rows seek to real offsets. Seeded on a couple of tracks; others start
 *  empty (the "Generate chapters" affordance), and `suggest_chapters` fills them
 *  from the segment timeline on demand. */
const CHAPTERS: Record<string, Chap[]> = {
  m1a: [
    { start_ms: 0, end_ms: 8000, title: "Intro & agenda", summary: "Kicking off and setting the agenda." },
    { start_ms: 8000, end_ms: 20000, title: "Roadmap discussion", summary: "Walking through the project roadmap." },
  ],
};

/** Synthesize chapters from a track's segments (the dev mock's stand-in for the
 *  daemon's LLM step): group the segment starts into a few coarse spans so the
 *  rows seek to real offsets. */
function mockChaptersFor(trackId: string): Chap[] {
  const segs = SEGMENTS[trackId] ?? [];
  if (!segs.length) return [];
  const titles = ["Opening", "Main discussion", "Wrap-up"];
  const per = Math.max(1, Math.ceil(segs.length / titles.length));
  const out: Chap[] = [];
  for (let i = 0; i < segs.length; i += per) {
    const chunk = segs.slice(i, i + per);
    const idx = out.length;
    if (idx >= titles.length) break;
    out.push({
      start_ms: chunk[0].start_ms,
      end_ms: chunk[chunk.length - 1].end_ms,
      title: titles[idx],
      summary: chunk[0].text.slice(0, 60),
    });
  }
  // Fill each end from the next start so they tile, like the daemon does.
  for (let i = 0; i < out.length - 1; i++) out[i].end_ms = out[i + 1].start_ms;
  return out;
}

/** A meeting track recording: shares `meeting_id` + `meeting_name`, tagged with
 *  its `track`, transcript derived from its segments. */
function track(
  id: string, meetingId: string, name: string, trackKind: "mic" | "system",
  daysAgo: number, h: number, m: number, segs: Seg[], tagIds: number[],
  // Extra per-track fields (e.g. seeded `entities`), merged after the meeting
  // fields so a track can carry its own enrichment in the mock.
  extra: Record<string, unknown> = {},
): Record<string, unknown> {
  const durMs = segs.length ? segs[segs.length - 1].end_ms : 0;
  const diarized = segs.some((s) => s.speaker != null);
  return rec(id, daysAgo, h, m, durMs, name, tagIds, false, trackTranscript(segs), {
    meeting_id: meetingId, track: trackKind, meeting_name: name, diarized, ...extra,
  });
}

// A deliberately long transcript (the synthetic paragraphs repeated) so the
// detail pane scrolls far enough to exercise the "back to top" button and the
// long-content layout.
const LONG = Array.from({ length: 12 }, (_, i) => `${i + 1}. ${i % 2 ? P1 : P2}`).join("\n\n");
const MORE_TITLES = [
  "Sprint retro notes", "Customer call summary", "Reading notes — chapter 3",
  "Weekend project ideas", "Standup follow-ups", "Book club discussion",
  "Travel planning memo", "Recipe dictation", "Workout log", "Lecture: intro",
  "1:1 talking points", "Release checklist", "Brainstorm: naming", "Daily journal",
  "Voicemail draft", "Errand list", "Meeting prep", "Idea: side feature",
  "Research summary", "Phone call notes",
];
/** A bigger batch of synthetic recordings (r13…) so the list is comfortably
 *  scrollable, spanning every type — single / favorite / in-place — with a few
 *  very long transcripts for the detail-pane scroll + back-to-top test. Fully
 *  synthetic; index-driven (no randomness) so the mock is stable across reloads. */
function moreRecordings(): Array<Record<string, unknown>> {
  const tagSets = [[1], [2], [3], [4], [1, 2], [2, 3], [3, 4], [1, 4], [], [1, 3]];
  const out: Array<Record<string, unknown>> = [];
  for (let i = 0; i < 30; i++) {
    const id = `r${String(i + 13).padStart(2, "0")}`;
    const daysAgo = 7 + Math.floor(i / 3); // spread across older days
    const h = 8 + (i % 12);
    const m = (i * 7) % 60;
    const favorite = i % 5 === 0;
    const pinned = i % 11 === 4; // a couple of pinned rows for preview
    const inPlace = i % 7 === 3;
    const isLong = i % 6 === 2;
    const title = MORE_TITLES[i % MORE_TITLES.length];
    const tags = tagSets[i % tagSets.length];
    const dur = 6000 + (i % 9) * 4200 + (isLong ? 360000 : 0);
    const transcript = isLong
      ? `${title} — extended sample.\n\n${LONG}`
      : `${title}. Placeholder transcript for layout testing.\n\n${i % 2 ? PARA : P1}`;
    const extra: Record<string, unknown> = {};
    if (inPlace) extra.in_place = true;
    if (pinned) extra.pinned = true;
    out.push(rec(id, daysAgo, h, m, dur, title, tags, favorite, transcript, extra));
  }
  return out;
}

const RECORDINGS: Array<Record<string, unknown>> = [
  // r01 carries a couple of extracted entities so the sidebar's browse-by-entity
  // facet renders without a backend; "Ada Lovelace" is shared with m1a below so
  // its facet count reads 2 (distinct recordings, not mentions).
  rec("r01", 0, 15, 11, 12200, "Sample voice note", [1], true, `Placeholder transcript used to render the preview without a backend.\n\n${PARA}`, {
    entities: [
      { kind: "person", value: "Ada Lovelace" },
      { kind: "org", value: "ACME Corp" },
      { kind: "topic", value: "project roadmap" },
    ],
    entities_model: "phi3:mini",
  }),
  // Meeting 1 (day 0, 10:05) — two tracks kept adjacent so the list groups them.
  track("m1a", "m1", "Product sync call", "mic", 0, 10, 5, M1_MIC, [1], {
    entities: [{ kind: "person", value: "Ada Lovelace" }],
    entities_model: "phi3:mini",
  }),
  track("m1b", "m1", "Product sync call", "system", 0, 10, 5, M1_SYS, [1]),
  // Meeting 2 (day 1, 09:30).
  track("m2a", "m2", "Design critique", "mic", 1, 9, 30, M2_MIC, [1, 3]),
  track("m2b", "m2", "Design critique", "system", 1, 9, 30, M2_SYS, [1, 3]),
  rec("r02", 1, 1, 43, 5900, "Weekly standup recap", [1], false, `Mock standup notes for layout testing.\n\n${P1}`),
  rec("r03", 1, 1, 40, 18000, "Project kickoff notes", [1, 3], false, `Sample meeting notes. The quick brown fox jumps over the lazy dog.\n\n${PARA}`),
  rec("r04", 1, 1, 23, 8400, "Grocery list memo", [2], true, "Eggs, milk, bread, coffee, olive oil, and a bag of rice. Mock content for layout testing only.", { in_place: true }),
  rec("r05", 1, 1, 21, 6900, "Podcast idea brainstorm", [3], false, `A few placeholder ideas for a future episode.\n\n${P1}`),
  rec("r06", 1, 1, 7, 20700, "Interview practice run", [2], false, `Tell me about yourself — sample answer text for the preview mock.\n\n${PARA}`),
  rec("r07", 1, 1, 6, 13800, "Lecture summary", [3], false, `Chapter one covers the basics and a couple of worked examples.\n\n${P2}`),
  rec("r08", 1, 1, 6, 7300, "Quick reply dictated in-place", [2], false, "Typed straight into the chat box via in-place dictation. Mock content for layout testing only.", { in_place: true }),
  rec("r09", 6, 19, 57, 13700, "Bug triage discussion", [1], false, `Reviewed a few sample issues. Mock transcript, no real data.\n\n${PARA}`),
  rec("r10", 6, 12, 32, 15500, "Design review notes", [1, 3], false, `Feedback on the sample mockups. Placeholder content only.\n\n${P2}`),
  rec("r11", 6, 1, 50, 215000, "Two-person conversation sample", [2, 4], true, CONVERSATION),
  rec("r12", 6, 16, 44, 53300, "Reading list voice memo", [4], false, `A longer placeholder note covering a few unrelated sample topics.\n\n${PARA}`),
  ...moreRecordings(),
  // Meetings scattered deeper in the list (days 4 / 8 / 11 / 14). list_recordings
  // sorts by start time, so these land at their date — spread through the scroll,
  // not clustered. Each meeting's two tracks share a timestamp and stay adjacent,
  // so the list folds each into one group.
  track("m4a", "m4", "Onboarding sync", "mic", 4, 11, 15, M4_MIC, [3]),
  track("m4b", "m4", "Onboarding sync", "system", 4, 11, 15, M4_SYS, [3]),
  track("m5a", "m5", "Cycle retro", "mic", 8, 16, 20, M5_MIC, [1]),
  track("m5b", "m5", "Cycle retro", "system", 8, 16, 20, M5_SYS, [1]),
  track("m3a", "m3", "Quarterly planning", "mic", 11, 14, 0, M3_MIC, [1, 3]),
  track("m3b", "m3", "Quarterly planning", "system", 11, 14, 0, M3_SYS, [1, 3]),
  track("m6a", "m6", "Client check-in", "mic", 14, 10, 45, M6_MIC, [2]),
  track("m6b", "m6", "Client check-in", "system", 14, 10, 45, M6_SYS, [2]),
];

// Seed pending auto-tag suggestions on most standalone recordings so the
// suggestion chips (approve / dismiss), the queue's tagging step, and the detail
// pane's "Suggestions pending" provenance line are all visible in the preview
// without running an LLM. Deterministic + index-driven (no randomness) so the
// mock is stable across reloads; ~1 in 6 rows is left empty so the no-suggestions
// state still renders. Meeting tracks are skipped. A suggested name is never one
// the row already carries as a real tag (you don't suggest a tag it already has).
const SUGGESTION_POOL = [
  "follow-up", "important", "draft", "review", "research", "client",
  "urgent", "q3-planning", "design", "bug", "decision", "action-items",
];
RECORDINGS.forEach((r, i) => {
  if (r.meeting_id != null) return; // skip meeting tracks (grouped UI)
  if (i % 6 === 5) return; // leave some without, for contrast
  const have = new Set((r.tags as Tag[]).map((t) => t.name));
  const want = 1 + (i % 3); // 1–3 suggestions per row
  const picks: string[] = [];
  for (let k = 0; picks.length < want && k < SUGGESTION_POOL.length; k++) {
    const name = SUGGESTION_POOL[(i * 2 + k) % SUGGESTION_POOL.length];
    if (!have.has(name) && !picks.includes(name)) picks.push(name);
  }
  r.tag_suggestions = picks;
});

/** A short, speech-shaped fake WAV synthesized once and shared by every recording
 *  (returned via the mocked convertFileSrc), so WaveSurfer has audio to draw —
 *  no binary committed to the repo. */
let wavUrl: string | null = null;
function fakeWavUrl(): string {
  if (wavUrl) return wavUrl;
  const sr = 8000;
  const n = sr * 6; // 6 seconds, mono, 16-bit PCM
  const buf = new ArrayBuffer(44 + n * 2);
  const dv = new DataView(buf);
  const str = (off: number, s: string) => { for (let i = 0; i < s.length; i++) dv.setUint8(off + i, s.charCodeAt(i)); };
  str(0, "RIFF"); dv.setUint32(4, 36 + n * 2, true); str(8, "WAVE");
  str(12, "fmt "); dv.setUint32(16, 16, true); dv.setUint16(20, 1, true); dv.setUint16(22, 1, true);
  dv.setUint32(24, sr, true); dv.setUint32(28, sr * 2, true); dv.setUint16(32, 2, true); dv.setUint16(34, 16, true);
  str(36, "data"); dv.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) {
    const t = i / sr;
    const word = Math.max(0, Math.sin(t * 3.1)) ** 2; // bursts ≈ words
    const syllable = 0.55 + 0.45 * Math.sin(t * 38); // intra-word flutter
    const s = (Math.random() * 2 - 1) * word * syllable * 0.85;
    dv.setInt16(44 + i * 2, Math.max(-1, Math.min(1, s)) * 32767, true);
  }
  wavUrl = URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
  return wavUrl;
}

// Fresh-install defaults — mirrors `Config::default()` from phoneme-core so the
// preview shows the out-of-the-box experience: bundled local Whisper, AI post-
// processing off, no custom hotkeys, the four built-in Playbook entries plus the
// single "default" recipe, Catppuccin Mocha, mouse/Tab navigation (no vim/arrow),
// live preview / overlay / REST / semantic search all off. (Generated from the
// Rust default via the config dump test; keep it in sync if defaults change.)
// `preview_whisper` and `in_place.stt` are absent by default (Option = None).
// Mutable: write_config replaces it and read_config returns it, so Settings
// round-trips. The seeded recordings/tags below are demo data, independent of
// this config, so the rest of the app stays explorable.
let config: Record<string, unknown> = {
  whisper: {
    mode: "bundled_download", external_url: "http://127.0.0.1:5809", model_path: "",
    bundled_server_port: 5809, bundled_server_args: [], timeout_secs: 3600,
    initial_prompt: "Voice memo. Common markers: Action Item:, Task:, To-do:, Follow up:, Decision:, Idea:, Question:, Reminder:.", provider: "local", api_key: "", model: "", api_url: "",
    use_own_bundled_server: false, low_confidence_threshold: 0.6,
  },
  in_place: {
    cleanup: "fast", full_pipeline: false, type_first: false, save_to_library: true,
    type_mode: "type", app_overrides: {}, app_context: false, app_context_denylist: [], stream_type: false,
    // Empty map = the built-in command set; enabled by default (today's behavior).
    voice_commands: {}, voice_commands_enabled: true,
  },
  recording: { audio_dir: "~/Documents/phoneme/audio", sample_rate: 16000, channels: 1, silence_threshold_dbfs: -45.0, silence_window_ms: 3000, max_duration_secs: 10800, input_device: "default", source: "microphone", pre_roll_ms: 1500, streaming_preview: false, auto_stop_on_silence: false, meeting_preview: "toggle", meeting_preview_own_server: false, normalize: false, normalize_target_dbfs: -1.0, preview_adaptive: true, preview_reveal_words_per_sec: 12.0, preview_idle_ms: 2500, preview_waveform: true },
  hook: { commands: ["powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-stdout.ps1"], timeout_secs: 30, webhook_url: null, run_on_transcribe: true, keyword_rules: [
    { pattern: "Action Item:", command: "powershell -NoProfile -Command \"$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\\phoneme-tasks.md') -Value ('- '+$d.transcript)\"", case_sensitive: false },
    { pattern: "Idea:", command: "powershell -NoProfile -Command \"$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\\phoneme-ideas.md') -Value ('- '+$d.transcript)\"", case_sensitive: false },
  ] },
  webhook: { allow_private_network: false, allow_http: false, hmac_secret: "", custom_headers: {} },
  hotkey: { enabled: false, combo: "Ctrl+Alt+Space", mode: "hold" },
  in_place_hotkey: { enabled: false, combo: "Ctrl+Alt+I", mode: "hold" },
  meeting_hotkey: { enabled: false, combo: "Ctrl+Alt+M", mode: "toggle" },
  hotkeys: [
    // Disabled-by-default examples showing recipe-bearing custom hotkeys.
    { id: "example-journal", label: "Example: journal note", enabled: false, combo: "Ctrl+Alt+J", mode: "hold", action: "record",
      recipe_id: "journal_note", whisper_model: "", in_place: { full_pipeline: false, type_mode: "type" } },
    { id: "example-prompt", label: "Example: dictate → prompt", enabled: false, combo: "Ctrl+Alt+P", mode: "hold", action: "in_place",
      recipe_id: "prompt_capture", whisper_model: "", in_place: { full_pipeline: true, type_mode: "type" } },
    { id: "example-meeting-notes", label: "Example: meeting notes", enabled: false, combo: "Ctrl+Alt+M", mode: "hold", action: "record",
      recipe_id: "meeting_notes", whisper_model: "", in_place: { full_pipeline: false, type_mode: "type" } },
  ],
  playbook: [
    { id: "cleanup", name: "Cleanup", description: "Tidy stutters, repetitions, and phonetic slips while keeping the original tone.", builtin: true, kind: "transform", target: "",
      llm: { provider: "", model: "", prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "title", name: "Title", description: "Generate a short title for the recording.", builtin: true, kind: "enrichment", target: "title",
      llm: { provider: "", model: "", prompt: "You title voice-note transcripts. Reply with ONLY a short title for the transcript: at most 8 words, plain text, no quotes, no trailing punctuation, no preamble.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "summary", name: "Summary", description: "Summarize the transcript into a few clear bullet points.", builtin: true, kind: "enrichment", target: "summary",
      llm: { provider: "", model: "", prompt: "Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "auto_tag", name: "Auto-tag", description: "Suggest tags for the recording (you approve before they apply).", builtin: true, kind: "enrichment", target: "tags",
      llm: { provider: "", model: "", prompt: "Suggest a few short topical tags for this transcript. Reply with ONLY a comma-separated list of lowercase tags, no preamble.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "prompt_polish", name: "Prompt polish", description: "Reshape a rough dictation into a clean, well-structured LLM prompt.", builtin: false, kind: "transform", target: "",
      llm: { provider: "", model: "", prompt: "Rewrite the following dictation into a single clear, well-structured prompt for an AI assistant. Keep the intent; fix grammar; output only the prompt.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "action_items", name: "Action items", description: "Pull any action items out of the transcript into a custom field.", builtin: false, kind: "enrichment", target: "custom:action_items",
      llm: { provider: "", model: "", prompt: "List any action items from this transcript as a short bulleted list. If there are none, reply 'None'.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "journal", name: "Append to journal", description: "A Hook step (no AI): append the transcript to a daily journal file.", builtin: false, kind: "hook", target: "",
      llm: { provider: "", model: "", prompt: "", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "powershell -NoProfile -Command \"$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\\phoneme-journal.md') -Value $d.transcript\"", webhook_url: "", timeout_secs: 60 } },
    { id: "formalize", name: "Formalize", description: "Rewrite the transcript in a polished, professional tone.", builtin: false, kind: "transform", target: "",
      llm: { provider: "", model: "", prompt: "Rewrite the following transcript in a clear, professional tone. Keep all meaning; fix grammar and remove filler. Output only the rewritten text.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "bulletize", name: "Bulletize", description: "Condense the transcript into concise bullet points.", builtin: false, kind: "transform", target: "",
      llm: { provider: "", model: "", prompt: "Condense the following transcript into concise, well-organized bullet points capturing every key point. Output only the bullets.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "sentiment", name: "Sentiment", description: "Tag the overall sentiment of the transcript into a custom field.", builtin: false, kind: "enrichment", target: "custom:sentiment",
      llm: { provider: "", model: "", prompt: "Classify the overall sentiment of this transcript as exactly one word: Positive, Neutral, or Negative. Reply with only that word.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "keywords", name: "Keywords", description: "Extract the key topics from the transcript into a custom field.", builtin: false, kind: "enrichment", target: "custom:keywords",
      llm: { provider: "", model: "", prompt: "Extract the 3-7 most important topics or keywords from this transcript. Reply with only a comma-separated list, lowercase.", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "", webhook_url: "", timeout_secs: 60 } },
    { id: "todo_capture", name: "Capture to-dos", description: "A keyword-triggered Hook: when the transcript contains \"Todo:\", append it to a to-do file.", builtin: false, kind: "hook", target: "",
      llm: { provider: "", model: "", prompt: "", api_url: "", api_key: "", timeout_secs: 300 }, hook: { command: "powershell -NoProfile -Command \"$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\\phoneme-todos.md') -Value ('- '+$d.transcript)\"", webhook_url: "", timeout_secs: 60, keyword: "Todo:", case_sensitive: false, required: false } },
  ],
  recipes: [
    { id: "default", name: "Default pipeline", description: "What every normal recording runs: cleanup, then title, summary, and tag suggestions.", builtin: true, steps: ["cleanup", "title", "summary", "auto_tag"] },
    { id: "prompt_capture", name: "Dictate → prompt", description: "Clean up the dictation, then reshape it into a polished LLM prompt.", builtin: false, steps: ["cleanup", "prompt_polish"] },
    { id: "meeting_notes", name: "Meeting notes", description: "Clean up, then summarize, pull action items, and tag — a full notes pass.", builtin: false, steps: ["cleanup", "summary", "action_items", "auto_tag"] },
    { id: "journal_note", name: "Journal note", description: "Clean up the dictation, then append it to your daily journal file.", builtin: false, steps: ["cleanup", "journal"] },
  ],
  playbook_migrated: false,
  hooks_migrated: false,
  tray: { show_on_startup: true, minimize_to_tray: true, start_at_login: false },
  editor: { vim_mode: false, vimrc: "", vimrc_path: "", resync_views_on_edit: true },
  diarization: { provider: "none", local_model_path: "", models_dir: "", solo_one_speaker: false, merge_gap_secs: 0.25, speaker_keep_threshold: 0.0000001, reconstruct_method: "smoothed", reconstruct_method_epsilon: 0.1, preload_at_startup: false },
  daemon: { log_level: "info", log_max_size_mb: 10, log_max_files: 5, pipe_name: "phoneme-daemon" },
  interface: {
    strip_titlebar: false,
    format_24h: false,
    date_day_first: false,
    theme: "catppuccin-mocha",
    visible_columns: ["day", "time", "duration", "status", "transcript"],
    column_widths: ["100px", "60px", "60px", "100px", "1fr"],
    preview_overlay: false,
    recording_indicator: false,
    vim_nav: false,
    arrow_nav: false,
    animation_speed: "normal",
    cursor_animation: "off",
    ui_font: "",
    ui_font_size: 14,
    step_notifications: true,
    quit_stops_daemon: true,
  },
  llm_post_process: { enabled: false, provider: "none", api_key: "", api_url: "", model: "llama3.2:3b", prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.", timeout_secs: 30, autostart_ollama: true },
  summary: { auto: false, provider: "", api_key: "", api_url: "", model: "", prompt: "Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble." },
  auto_tag: { auto: false, provider: "", api_key: "", api_url: "", model: "", prompt: "You tag voice-note transcripts. Suggest concise topical tags (1-3 words each). Reuse tags from the EXISTING TAGS list when they genuinely fit, AND coin new tags for topics no existing tag covers — a good answer usually mixes both. Reply with ONLY a JSON array of tag-name strings — no preamble, no explanations.", max_tags: 5, auto_accept_existing: false },
  title: { enabled: true, use_llm: false, provider: "", api_key: "", api_url: "", model: "", prompt: "You title voice-note transcripts. Reply with ONLY a short title for the transcript: at most 8 words, plain text, no quotes, no trailing punctuation, no preamble." },
  semantic_search: { enabled: false, model_dir: "", max_tokens: 256, pooling: "mean", token_type_ids: true, query_prefix: "", passage_prefix: "" },
  retention: { delete_audio: false },
  rest_api: { enabled: false, port: 3737 },
};

/** Next free tag id (max existing + 1). */
function newTagId(): number {
  return TAGS.reduce((mx, t) => Math.max(mx, t.id), 0) + 1;
}
/** A recording's live tag array (mutated in place by attach / detach). */
function recTags(r: Record<string, unknown>): Tag[] {
  return r.tags as Tag[];
}

// Daemon-event emission. The real tray bridge re-emits every daemon state change
// as the Tauri event "daemon-event", and the whole UI re-fetches off it (see
// services/events.ts). mockIPC doesn't deliver events, so we capture each
// "daemon-event" listener's callback as it subscribes (below) and invoke them
// ourselves — tag edits then refresh the sidebar + Tag Manager live, exactly as
// in the native app, instead of only the surface that made the edit.
const daemonListeners: Array<(payload: unknown) => void> = [];
function emitDaemon(evt: Record<string, unknown>): void {
  // Defer a microtask so the broadcast lands AFTER the triggering invoke resolves.
  void Promise.resolve().then(() => {
    for (const cb of daemonListeners) {
      try {
        cb({ event: "daemon-event", id: 0, payload: evt });
      } catch {
        /* a dead listener must not break the others */
      }
    }
  });
}

/** Whole-meeting digests generated in preview, keyed by meeting_id. Starts empty
 *  (no digest until the user clicks Generate), mirroring a fresh daemon. */
const MEETING_DIGESTS: Record<string, { meeting_id: string; digest: string; digest_model: string | null }> = {};

/** Replace a non-empty `api_key`/secret string on `obj` with the mask placeholder. */
function maskKey(obj: unknown, field: string): void {
  if (obj && typeof obj === "object") {
    const rec = obj as Record<string, unknown>;
    if (typeof rec[field] === "string" && rec[field] !== "") rec[field] = MASKED_SECRET;
  }
}

/** Deep-clone `config` with every secret masked, mirroring the backend's
 *  `mask_config_secrets` (src-tauri/src/commands/mod.rs) so the dev preview's
 *  `read_config` never hands the renderer a real key — same sections, same
 *  placeholder. The stored config keeps its real values (like the on-disk file),
 *  so `write_config` can round-trip an unchanged, still-masked key. */
function maskedConfig(): Record<string, unknown> {
  const cfg = structuredClone(config) as Record<string, unknown>;
  for (const section of ["whisper", "llm_post_process", "summary", "auto_tag", "title", "preview_whisper"]) {
    maskKey(cfg[section], "api_key");
  }
  // The dictation STT key lives one level deeper (`in_place.stt.api_key`).
  maskKey((cfg.in_place as Record<string, unknown> | undefined)?.stt, "api_key");
  // The webhook HMAC signing key is a secret too (`webhook.hmac_secret`).
  maskKey(cfg.webhook, "hmac_secret");
  // Playbook entries each carry their own LLM key (`playbook[].llm.api_key`).
  if (Array.isArray(cfg.playbook)) {
    for (const entry of cfg.playbook as Array<Record<string, unknown>>) maskKey(entry.llm, "api_key");
  }
  return cfg;
}

function handle(cmd: string, args: Record<string, unknown>): unknown {
  const id = args.id as string | undefined;
  switch (cmd) {
    case "config_exists": return true;
    // Mask secrets exactly as the backend's read_config does, so the preview's
    // Settings sees the placeholder (not a real key) like the native app.
    case "read_config": return maskedConfig();
    // Persist edits in-memory so Settings round-trips: Save writes the whole
    // config back, and the next read_config (and the config:saved event the view
    // dispatches itself) reflects it — theme / cursor / nav changes apply live.
    case "write_config": { if (args.config) config = args.config as Record<string, unknown>; return undefined; }
    case "reload_config": return undefined;
    // Settings / wizard side-effects that don't apply in a browser: accept them.
    case "open_file":
    case "set_overlay":
    case "record_stop":
    case "wizard_download_diarization_model":
    case "wizard_pull_ollama_model":
    case "ollama_pull_model":
    case "ollama_delete_model":
    case "wizard_run_installer": return undefined;
    // Local-Ollama model manager: a stub install list so the manager renders in
    // the browser preview without a real Ollama.
    case "ollama_list_installed":
      return [
        { name: "llama3.2:3b", size: 2_019_393_189, modified_at: "2026-06-10T09:00:00Z" },
        { name: "phi3:mini", size: 2_318_920_000, modified_at: "2026-06-01T10:00:00Z" },
      ];
    case "record_start": return { id: "mock-rec" };
    case "list_recordings": {
      const f = (args.filter ?? {}) as Record<string, unknown>;
      // Sort newest-first like the real daemon does, so the date groups render in
      // order and the scattered meetings land at their real dates. A meeting's two
      // tracks share a timestamp; the stable sort keeps them adjacent for grouping.
      let rows = [...RECORDINGS].sort((a, b) =>
        String(b.started_at).localeCompare(String(a.started_at)),
      );
      // Pinned recordings float to the top, mirroring the daemon's
      // `pinned DESC` lead in the ORDER BY (stable within each group).
      rows.sort((a, b) => Number(!!b.pinned) - Number(!!a.pinned));
      if (f.favorite === true) rows = rows.filter((r) => r.favorite);
      if (f.pinned === true) rows = rows.filter((r) => r.pinned);
      else if (f.pinned === false) rows = rows.filter((r) => !r.pinned);
      if (f.in_place === true) rows = rows.filter((r) => r.in_place);
      if (f.kind === "single") rows = rows.filter((r) => r.meeting_id == null);
      if (f.kind === "meeting") rows = rows.filter((r) => r.meeting_id != null);
      const tagId = f.tag_id as number | undefined;
      if (tagId != null) rows = rows.filter((r) => (r.tags as Array<{ id: number }>).some((t) => t.id === tagId));
      // Tag-presence filter ("All Tags" = true, "Untagged" = false).
      if (f.tagged === true) rows = rows.filter((r) => (r.tags as unknown[]).length > 0);
      else if (f.tagged === false) rows = rows.filter((r) => (r.tags as unknown[]).length === 0);
      // Entity facet filter: keep recordings mentioning this exact entity value,
      // optionally pinned to one kind (mirrors the daemon's `entities` subquery).
      const entityValue = f.entity_value as string | undefined;
      if (entityValue != null) {
        const entityKind = f.entity_kind as string | undefined;
        rows = rows.filter((r) =>
          ((r.entities as Array<{ kind: string; value: string }> | undefined) ?? []).some(
            (e) => e.value === entityValue && (entityKind == null || e.kind === entityKind),
          ),
        );
      }
      return rows;
    }
    case "get_recording": return RECORDINGS.find((r) => r.id === id) ?? RECORDINGS[0];
    case "list_meeting": {
      // The session's tracks (mic before system), ordered as the merged view expects.
      const mid = args.meetingId as string | undefined;
      return RECORDINGS.filter((r) => r.meeting_id === mid)
        .sort((a, b) => String(a.track).localeCompare(String(b.track)));
    }
    case "list_tags": // sidebar: only tags attached to ≥1 recording
      return TAGS.filter((t) => RECORDINGS.some((r) => recTags(r).some((x) => x.id === t.id)));
    case "list_all_tags": return TAGS; // Tag Manager: every tag, including orphans
    case "tags_for": {
      // The UI passes `recordingId`; tolerate `id` too.
      const rid = (args.recordingId ?? args.id) as string | undefined;
      return (RECORDINGS.find((r) => r.id === rid)?.tags as unknown) ?? [];
    }
    case "tag_usage_counts": {
      const counts: Record<string, number> = {};
      for (const r of RECORDINGS) for (const t of recTags(r)) counts[String(t.id)] = (counts[String(t.id)] ?? 0) + 1;
      return counts;
    }
    // ── Tag mutations: mutate the in-memory catalog + broadcast the matching
    //    daemon event so every tag surface refreshes, like the real daemon. ──
    case "add_tag": {
      const name = String(args.name ?? "").trim();
      const existing = TAGS.find((t) => t.name.toLowerCase() === name.toLowerCase());
      if (existing) return existing; // lenient (the real daemon rejects duplicates)
      const tag: Tag = { id: newTagId(), name, color: (args.color as string | null) ?? null };
      TAGS.push(tag);
      emitDaemon({ event: "tag_created", id: tag.id });
      return tag;
    }
    case "update_tag": {
      const tag = TAGS.find((t) => t.id === (args.id as number));
      if (tag) {
        tag.name = String(args.name ?? tag.name);
        tag.color = (args.color as string | null) ?? null;
        emitDaemon({ event: "tag_updated", id: tag.id });
      }
      return tag ?? null;
    }
    case "delete_tag": {
      const tid = args.id as number;
      const i = TAGS.findIndex((t) => t.id === tid);
      if (i >= 0) TAGS.splice(i, 1);
      for (const r of RECORDINGS) {
        const ts = recTags(r);
        const j = ts.findIndex((t) => t.id === tid);
        if (j >= 0) ts.splice(j, 1);
      }
      emitDaemon({ event: "tag_deleted", id: tid });
      return undefined;
    }
    case "delete_recording": {
      const id = args.id as string;
      const i = RECORDINGS.findIndex((x) => x.id === id);
      if (i >= 0) RECORDINGS.splice(i, 1);
      emitDaemon({ event: "recording_deleted", id });
      return undefined;
    }
    case "set_pinned": {
      // Mutate the in-memory record so the pinned-first sort + sidebar "Pinned"
      // badge reflect it in the browser preview.
      const id = args.id as string;
      const rec = RECORDINGS.find((x) => x.id === id);
      if (rec) rec.pinned = !!args.pinned;
      return undefined;
    }
    case "delete_session": {
      const mid = args.meetingId as string;
      // Remove every track of the meeting, emitting one event per track (the
      // real daemon does the same), so list + counts reconcile in the preview.
      for (let i = RECORDINGS.length - 1; i >= 0; i--) {
        if (RECORDINGS[i].meeting_id === mid) {
          const { id } = RECORDINGS[i];
          RECORDINGS.splice(i, 1);
          emitDaemon({ event: "recording_deleted", id });
        }
      }
      return undefined;
    }
    case "rebuild_catalog": {
      // Preview no-op: report the current count so the Doctor button works
      // without wiping the demo dataset (the real daemon clears + re-imports).
      return { count: RECORDINGS.length };
    }
    case "attach_tag": {
      const r = RECORDINGS.find((x) => x.id === (args.recordingId as string));
      const tag = TAGS.find((t) => t.id === (args.tagId as number));
      if (r && tag && !recTags(r).some((t) => t.id === tag.id)) recTags(r).push(tag);
      emitDaemon({ event: "tag_attached", tag_id: args.tagId });
      return undefined;
    }
    case "detach_tag": {
      const r = RECORDINGS.find((x) => x.id === (args.recordingId as string));
      if (r) {
        const ts = recTags(r);
        const j = ts.findIndex((t) => t.id === (args.tagId as number));
        if (j >= 0) ts.splice(j, 1);
      }
      emitDaemon({ event: "tag_detached", tag_id: args.tagId });
      return undefined;
    }
    case "merge_tags": {
      const from = args.fromId as number, into = args.intoId as number;
      if (from !== into) {
        const intoTag = TAGS.find((t) => t.id === into);
        for (const r of RECORDINGS) {
          const ts = recTags(r);
          const j = ts.findIndex((t) => t.id === from);
          if (j >= 0) {
            ts.splice(j, 1);
            if (intoTag && !ts.some((t) => t.id === into)) ts.push(intoTag);
          }
        }
        const i = TAGS.findIndex((t) => t.id === from);
        if (i >= 0) TAGS.splice(i, 1);
        emitDaemon({ event: "tag_deleted", id: from });
      }
      return undefined;
    }
    // ── Tag suggestions (the ✨ chips): synthesize a few proposals so approve /
    //    dismiss / approve-all can be exercised too. ──
    case "suggest_tags": {
      const r = RECORDINGS.find((x) => x.id === id);
      if (r) {
        const have = new Set(recTags(r).map((t) => t.name));
        r.tag_suggestions = ["meeting", "follow-up", "important", "draft", "review"]
          .filter((n) => !have.has(n))
          .slice(0, 3);
        emitDaemon({ event: "tag_suggestions_updated", id });
      }
      return undefined;
    }
    // ── Entity extraction (the 🔎 chips): synthesize a few typed entities so the
    //    grouped chips render in dev. ──
    case "suggest_entities": {
      const r = RECORDINGS.find((x) => x.id === id);
      if (r) {
        r.entities = [
          { kind: "person", value: "Ada Lovelace" },
          { kind: "org", value: "ACME Corp" },
          { kind: "topic", value: "project roadmap" },
          { kind: "term", value: "RRF" },
        ];
        r.entities_model = "phi3:mini";
        emitDaemon({ event: "entities_updated", id });
      }
      return undefined;
    }
    // ── Auto-chapters (the 🗂 Chapters view): synthesize chapters from the
    //    track's segment timeline so the rows seek to real offsets, then broadcast
    //    so the view live-refreshes. ──
    case "suggest_chapters": {
      if (id) {
        CHAPTERS[id] = mockChaptersFor(id);
        const r = RECORDINGS.find((x) => x.id === id);
        if (r) r.chapters_model = "phi3:mini";
        emitDaemon({ event: "chapters_updated", id });
      }
      return undefined;
    }
    case "get_chapters": return id ? (CHAPTERS[id] ?? []) : [];
    // ── Whole-meeting digest (the merged-view digest card): store a canned digest
    //    and broadcast so the parent reloads + clears the "Generating…" state.
    //    The event carries `meeting_id` (not a recording id) per events.ts. ──
    case "rerun_meeting_digest": {
      const mid = args.meetingId as string;
      MEETING_DIGESTS[mid] = {
        meeting_id: mid,
        digest:
          "Demo whole-meeting digest: the mic and system tracks were synthesized into one " +
          "reading. Decisions, owners, and follow-ups would be summarized here.",
        digest_model: (args.model as string) || "phi3:mini",
      };
      emitDaemon({ event: "meeting_digest_updated", meeting_id: mid });
      return undefined;
    }
    case "get_meeting_digest": {
      const mid = args.meetingId as string;
      return MEETING_DIGESTS[mid] ?? null;
    }
    case "approve_tag_suggestion": {
      const r = RECORDINGS.find((x) => x.id === id);
      const name = String(args.name ?? "");
      let tag = TAGS.find((t) => t.name.toLowerCase() === name.toLowerCase());
      if (!tag) {
        tag = { id: newTagId(), name, color: null };
        TAGS.push(tag);
        emitDaemon({ event: "tag_created", id: tag.id });
      }
      if (r) {
        if (!recTags(r).some((t) => t.id === tag!.id)) recTags(r).push(tag);
        r.tag_suggestions = ((r.tag_suggestions as string[]) ?? []).filter((n) => n !== name);
        emitDaemon({ event: "tag_attached", tag_id: tag.id });
        emitDaemon({ event: "tag_suggestions_updated", id });
      }
      return tag;
    }
    case "dismiss_tag_suggestion": {
      const r = RECORDINGS.find((x) => x.id === id);
      if (r) {
        r.tag_suggestions = ((r.tag_suggestions as string[]) ?? []).filter((n) => n !== args.name);
        emitDaemon({ event: "tag_suggestions_updated", id });
      }
      return undefined;
    }
    case "clear_all_tag_suggestions": {
      let cleared = 0;
      for (const r of RECORDINGS) {
        const s = r.tag_suggestions as string[] | undefined;
        if (s && s.length) {
          cleared++;
          r.tag_suggestions = [];
          emitDaemon({ event: "tag_suggestions_updated", id: r.id });
        }
      }
      emitDaemon({ event: "all_tag_suggestions_cleared", cleared });
      return { cleared };
    }
    case "kind_counts": return {
      all: RECORDINGS.length,
      single: RECORDINGS.filter((r) => r.meeting_id == null).length,
      meeting: RECORDINGS.filter((r) => r.meeting_id != null).length,
      in_place: RECORDINGS.filter((r) => r.in_place).length,
      favorite: RECORDINGS.filter((r) => r.favorite).length,
      pinned: RECORDINGS.filter((r) => r.pinned).length,
      tagged: RECORDINGS.filter((r) => Array.isArray(r.tags) && (r.tags as unknown[]).length > 0).length,
      untagged: RECORDINGS.filter((r) => !Array.isArray(r.tags) || (r.tags as unknown[]).length === 0).length,
    };
    // The cross-recording entity facet: distinct (kind, value) across the seeded
    // recordings with their recording counts, kind- then value-sorted (mirrors the
    // daemon's `entity_facets`). Powers the sidebar's browse-by-entity section.
    case "list_all_entities": {
      const counts = new Map<string, { kind: string; value: string; count: number }>();
      for (const r of RECORDINGS) {
        for (const e of ((r.entities as Array<{ kind: string; value: string }> | undefined) ?? [])) {
          const key = `${e.kind} ${e.value}`;
          const row = counts.get(key);
          if (row) row.count += 1;
          else counts.set(key, { kind: e.kind, value: e.value, count: 1 });
        }
      }
      return [...counts.values()].sort(
        (a, b) => a.kind.localeCompare(b.kind) || a.value.localeCompare(b.value),
      );
    }
    case "get_segments": return id ? (SEGMENTS[id] ?? []) : [];
    case "get_words": return [];
    case "get_original_transcript":
    case "get_clean_transcript": return null;
    case "list_ai_activity": return [];
    case "list_queue": return [];
    case "queue_counts": return { pending: 0, processing: 0, done: 0, failed: 0 };
    case "queue_paused":
    case "set_queue_paused": return { paused: false };
    case "daemon_status": return { running: true, pid: 4242 };
    case "run_doctor": return [{ name: "Mock mode", ok: true, detail: "Browser preview — no daemon", fix_action: null }];
    case "semantic_search":
    case "more_like_this": return [];
    case "list_profiles": return ["default"];
    case "list_profiles_detailed": return [{ name: "default", modified_ms: null }];
    // Event plumbing: accept listen/unlisten so subscribe() resolves. For the
    // "daemon-event" stream we also capture the listener's callback (Tauri stores
    // it on window as `_<handler>` via transformCallback) so emitDaemon can drive
    // it — tag edits then refresh every subscribed surface, like the real bridge.
    case "plugin:event|listen": {
      if (args.event === "daemon-event" && typeof args.handler === "number") {
        const cb = (window as unknown as Record<string, unknown>)["_" + args.handler];
        if (typeof cb === "function") daemonListeners.push(cb as (p: unknown) => void);
      }
      return ++eventId;
    }
    case "plugin:event|unlisten":
    case "plugin:event|emit":
    case "plugin:event|emit_to": return undefined;
    // Saved searches (catalog-backed). Preview starts with none.
    case "list_saved_searches": return [];
    case "upsert_saved_search": return undefined;
    case "delete_saved_search": return { removed: true };
    // Named-speaker recognition (#9). Preview-only stubs so the suggestion chip +
    // Speaker Library render; real matching happens against voiceprints in the daemon.
    case "recognize_speakers":
      if (args.id === "r11")
        return [{ speaker_label: 2, name: "Alex Rivera", named_voice_id: "nv_demo", score: 0.82 }];
      // A meeting's system track, so the merged-view banner is visible in preview.
      if (args.id === "m2b")
        return [{ speaker_label: 3, name: "Jordan Lee", named_voice_id: "nv_demo2", score: 0.77 }];
      return [];
    case "dismiss_speaker_suggestion":
    case "rename_named_voice":
      return undefined;
    case "list_named_voices":
      return [
        { id: "nv_demo", name: "Alex Rivera", samples: 3 },
        { id: "nv_demo2", name: "Sam Chen", samples: 1 },
      ];
    case "merge_named_voices": return { merged: true };
    case "forget_named_voice": return { removed: true };
    // Commands the real backend serves; without these stubs they fall through to
    // `default` and the preview can't populate a device picker, the wizard's
    // "Test connection" button, the overlay source toggle, or window-state saves.
    // Return the backend's shapes so those surfaces behave natively.
    case "list_input_devices": return ["default", "Microphone (mock)", "Headset (mock)"];
    case "wizard_test_whisper": return { ok: true, message: "HTTP 200" };
    case "set_preview_source": // overlay source toggle: a no-op in the browser.
    case "save_window_state": return undefined; // no native windows to persist.
    default: return null;
  }
}

let eventId = 0;

/** Install the mock, but only in a browser without the real Tauri runtime. */
export function installTauriDevMock(): void {
  if (!import.meta.env.DEV) return;
  if ((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) return;
  mockIPC((cmd, payload) => handle(cmd, (payload ?? {}) as Record<string, unknown>));
  // mockIPC doesn't provide convertFileSrc; point every audio path at the shared
  // synthetic WAV so the WaveformPlayer has something to render.
  const internals = (window as unknown as { __TAURI_INTERNALS__: Record<string, unknown> }).__TAURI_INTERNALS__;
  internals.convertFileSrc = () => fakeWavUrl();
  // eslint-disable-next-line no-console
  console.info("[phoneme] Tauri dev mock active — canned data, no daemon (browser preview).");
}

installTauriDevMock();
