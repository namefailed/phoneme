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
 * SAFETY — never affects the real app:
 *   - Installed ONLY when this is a Vite dev build (`import.meta.env.DEV`) AND
 *     there is no real Tauri runtime. In `cargo tauri dev` and production builds
 *     `window.__TAURI_INTERNALS__` is injected by Tauri, so the mock is skipped.
 *   - In a production build `import.meta.env.DEV` is statically false, so the whole
 *     block (and the `@tauri-apps/api/mocks` import) is dead-code-eliminated.
 *
 * It mocks only the commands the UI calls on mount / common interactions; events
 * are accepted (so `listen()` resolves) but never emitted.
 */
import { mockIPC } from "@tauri-apps/api/mocks";

const TAGS = [
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
    diarized: id === "r11",
    user_edited: false,
    tags: tagIds.map((t) => TAGS.find((x) => x.id === t)).filter(Boolean),
    speaker_names: [],
    tag_suggestions: [],
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

const RECORDINGS: Array<Record<string, unknown>> = [
  rec("r01", 0, 15, 11, 12200, "Sample voice note", [1], true, `Placeholder transcript used to render the preview without a backend.\n\n${PARA}`),
  rec("r02", 1, 1, 43, 5900, "Weekly standup recap", [1], false, `Mock standup notes for layout testing.\n\n${P1}`),
  rec("r03", 1, 1, 40, 18000, "Project kickoff notes", [1, 3], false, `Sample meeting notes. The quick brown fox jumps over the lazy dog.\n\n${PARA}`),
  rec("r04", 1, 1, 23, 8400, "Grocery list memo", [2], true, "Eggs, milk, bread, coffee, olive oil, and a bag of rice. Mock content for layout testing only."),
  rec("r05", 1, 1, 21, 6900, "Podcast idea brainstorm", [3], false, `A few placeholder ideas for a future episode.\n\n${P1}`),
  rec("r06", 1, 1, 7, 20700, "Interview practice run", [2], false, `Tell me about yourself — sample answer text for the preview mock.\n\n${PARA}`),
  rec("r07", 1, 1, 6, 13800, "Lecture summary", [3], false, `Chapter one covers the basics and a couple of worked examples.\n\n${P2}`),
  rec("r08", 1, 1, 6, 7300, "Daily journal entry", [2], false, `Today was a normal day. Fake journal text for the demo.\n\n${P1}`),
  rec("r09", 6, 19, 57, 13700, "Bug triage discussion", [1], false, `Reviewed a few sample issues. Mock transcript, no real data.\n\n${PARA}`),
  rec("r10", 6, 12, 32, 15500, "Design review notes", [1, 3], false, `Feedback on the sample mockups. Placeholder content only.\n\n${P2}`),
  rec("r11", 6, 1, 50, 215000, "Two-person conversation sample", [2, 4], true, CONVERSATION),
  rec("r12", 6, 16, 44, 53300, "Reading list voice memo", [4], false, `A longer placeholder note covering a few unrelated sample topics.\n\n${PARA}`),
];

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

// Mirrors the maintainer's real config so the preview matches the actual app.
// SCRUBBED for the public repo: the llm_post_process API key (a DPAPI secret) is
// blanked, and model paths are de-usernamed (C:\Users\<name> -> ~). Mutable:
// write_config replaces it and read_config returns it, so Settings round-trips.
const STT = (port: number, model: string) => ({
  mode: "bundled_model", external_url: "http://127.0.0.1:5809",
  model_path: `~/AppData/Local/phoneme/data/models/${model}`,
  bundled_server_port: port, bundled_server_args: [], timeout_secs: 9999,
  language: "en", provider: "local", api_key: "", model: "", api_url: "",
  use_own_bundled_server: false,
});
let config: Record<string, unknown> = {
  whisper: { ...STT(5809, "ggml-medium.en.bin"), mode: "bundled_download" },
  preview_whisper: STT(5810, "ggml-base.en.bin"),
  in_place: {
    cleanup: "fast", full_pipeline: false, type_first: true, save_to_library: true,
    type_mode: "paste", app_context: true, app_context_denylist: ["code"], stream_type: false,
    stt: STT(5810, "ggml-base.en.bin"), app_overrides: {},
  },
  recording: { audio_dir: "~/Documents/phoneme/audio", sample_rate: 16000, channels: 1, silence_threshold_dbfs: -50.0, silence_window_ms: 3000, max_duration_secs: 300, input_device: "default", source: "microphone", pre_roll_ms: 1000, streaming_preview: true, auto_stop_on_silence: false, meeting_preview: "toggle", meeting_preview_own_server: true, normalize: false, normalize_target_dbfs: -1.0, preview_adaptive: true, preview_reveal_words_per_sec: 12.0, preview_idle_ms: 10000, preview_waveform: true },
  hook: { commands: ['powershell -Command "$d=($input|Out-String|ConvertFrom-Json); Set-Clipboard -Value $d.transcript"'], timeout_secs: 30, run_on_transcribe: true, keyword_rules: [] },
  webhook: { allow_private_network: false, allow_http: false, hmac_secret: "", custom_headers: {} },
  hotkey: { enabled: true, combo: "Alt+V", mode: "hold" },
  in_place_hotkey: { enabled: true, combo: "Shift+Alt+V", mode: "hold" },
  meeting_hotkey: { enabled: true, combo: "Ctrl+Alt+V", mode: "hold" },
  tray: { show_on_startup: true, minimize_to_tray: true, start_at_login: true },
  editor: { vim_mode: true, vimrc: "", vimrc_path: "", resync_views_on_edit: true },
  diarization: { provider: "local", local_model_path: "", models_dir: "", solo_one_speaker: false, merge_gap_secs: 0.25, speaker_keep_threshold: 0.0000001, reconstruct_method: "smoothed", reconstruct_method_epsilon: 0.1 },
  daemon: { log_level: "trace", log_max_size_mb: 10, log_max_files: 5, pipe_name: "phoneme-daemon" },
  interface: {
    strip_titlebar: false,
    format_24h: true,
    theme: "catppuccin-mocha",
    visible_columns: ["day", "time", "duration", "title", "status", "model", "cleanup_model", "user_edited", "transcript"],
    column_widths: ["40px", "117px", "66px", "84px", "159px", "156px", "176px", "145px", "390px", "80px", "69px", "124px", "408px", "1fr"],
    preview_overlay: true,
    recording_indicator: false,
    vim_nav: true,
    arrow_nav: true,
    animation_speed: "normal",
    cursor_animation: "glide",
    ui_font: "Cascadia Code",
    ui_font_size: 12,
    step_notifications: true,
    quit_stops_daemon: true,
  },
  llm_post_process: { enabled: true, provider: "ollama", api_key: "", api_url: "http://127.0.0.1:11434/api/generate", model: "gemma3:4b", prompt: "Clean up the following transcript. Maintain original tone. Provide only the transcript and things pertaining to it, not your replies. Do not provide any meta information, clean up lists and make the information readable and pretty.", timeout_secs: 30, autostart_ollama: true },
  summary: { auto: true, provider: "", api_key: "", api_url: "", model: "", prompt: "Summarize the following transcript concisely as clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble. Provide only the summary, make sure the summary is readable and beautiful." },
  auto_tag: { auto: true, provider: "", api_key: "", api_url: "", model: "", prompt: "You tag voice-note transcripts. Suggest concise topical tags (1-2 words each). Reuse tags from the EXISTING TAGS list when they genuinely fit, AND coin new tags for topics no existing tag covers. Reply with ONLY a JSON array of tag-name strings — no preamble, no explanations.", max_tags: 10, auto_accept_existing: true },
  title: { enabled: true, use_llm: true, provider: "", api_key: "", api_url: "", model: "", prompt: "You title voice-note transcripts. Reply with ONLY a short title for the transcript: at most 8 words, plain text, no quotes, no trailing punctuation, no preamble. Provide only the title and make sure they're beautiful and clearly title the transcript." },
  semantic_search: { enabled: true, model_dir: "~/AppData/Local/phoneme/data/models/semantic", max_tokens: 256, pooling: "mean", token_type_ids: true, query_prefix: "", passage_prefix: "" },
  retention: { delete_audio: false },
  rest_api: { enabled: true, port: 3737 },
};

function handle(cmd: string, args: Record<string, unknown>): unknown {
  const id = args.id as string | undefined;
  switch (cmd) {
    case "config_exists": return true;
    case "read_config": return config;
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
    case "wizard_run_installer": return undefined;
    case "record_start": return { id: "mock-rec" };
    case "list_recordings": {
      const f = (args.filter ?? {}) as Record<string, unknown>;
      let rows = RECORDINGS;
      if (f.favorite === true) rows = rows.filter((r) => r.favorite);
      const tagId = f.tag_id as number | undefined;
      if (tagId != null) rows = rows.filter((r) => (r.tags as Array<{ id: number }>).some((t) => t.id === tagId));
      return rows;
    }
    case "get_recording": return RECORDINGS.find((r) => r.id === id) ?? RECORDINGS[0];
    case "list_meeting": return [];
    case "list_tags":
    case "list_all_tags": return TAGS;
    case "tags_for": return (RECORDINGS.find((r) => r.id === id)?.tags as unknown) ?? [];
    case "tag_usage_counts": return { "1": 2, "2": 3, "3": 4, "4": 2 };
    case "get_segments":
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
    // Event plumbing: accept listen/unlisten so subscribe() resolves; we never emit.
    case "plugin:event|listen": return ++eventId;
    case "plugin:event|unlisten":
    case "plugin:event|emit":
    case "plugin:event|emit_to": return undefined;
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
