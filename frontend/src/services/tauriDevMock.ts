/// <reference types="vite/client" />
/**
 * Dev-only Tauri IPC mock — lets phoneme render in a plain browser (the Claude
 * Code preview, or `vite` opened directly), where `window.__TAURI_INTERNALS__`
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
const LOREM = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore.";
const RECORDINGS: Array<Record<string, unknown>> = [
  rec("r01", 0, 15, 11, 12200, "Sample voice note", [1], true, "Placeholder transcript used to render the preview without a backend. " + LOREM),
  rec("r02", 1, 1, 43, 5900, "Weekly standup recap", [1], false, "Mock standup notes. " + LOREM),
  rec("r03", 1, 1, 40, 18000, "Project kickoff notes", [1, 3], false, "Sample meeting notes. The quick brown fox jumps over the lazy dog."),
  rec("r04", 1, 1, 23, 8400, "Grocery list memo", [2], true, "Eggs, milk, bread, coffee. Mock content for layout testing only."),
  rec("r05", 1, 1, 21, 6900, "Podcast idea brainstorm", [3], false, "A few placeholder ideas for a future episode. " + LOREM),
  rec("r06", 1, 1, 7, 20700, "Interview practice run", [2], false, "Tell me about yourself — sample answer text for the preview mock."),
  rec("r07", 1, 1, 6, 13800, "Lecture summary", [3], false, "Chapter one covers the basics. " + LOREM),
  rec("r08", 1, 1, 6, 7300, "Daily journal entry", [2], false, "Today was a normal day. Fake journal text for the demo."),
  rec("r09", 6, 19, 57, 13700, "Bug triage discussion", [1], false, "Reviewed a few sample issues. Mock transcript, no real data."),
  rec("r10", 6, 12, 32, 15500, "Design review notes", [1, 3], false, "Feedback on the sample mockups. Placeholder content only."),
  rec("r11", 6, 1, 50, 215000, "Two-person conversation sample", [2, 4], true, "[Speaker 1]: Hello, how are you today? [Speaker 2]: Doing well, thanks — just testing the diarized layout."),
  rec("r12", 6, 16, 44, 53300, "Reading list voice memo", [4], false, "A longer placeholder note covering a few unrelated sample topics. " + LOREM),
];

const CONFIG = {
  interface: {
    theme: "one-dark",
    vim_nav: true,
    arrow_nav: false,
    cursor_animation: "trail",
    animation_speed: "normal",
    ui_font: "",
    ui_font_size: 14,
    strip_titlebar: false,
    step_notifications: true,
    preview_overlay: false,
    recording_indicator: false,
    quit_stops_daemon: false,
  },
  semantic_search: { enabled: false },
};

function handle(cmd: string, args: Record<string, unknown>): unknown {
  const id = args.id as string | undefined;
  switch (cmd) {
    case "config_exists": return true;
    case "read_config": return CONFIG;
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
  // eslint-disable-next-line no-console
  console.info("[phoneme] Tauri dev mock active — canned data, no daemon (browser preview).");
}

installTauriDevMock();
