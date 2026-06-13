# 🎨 Frontend Developer Guide

Phoneme's frontend is a single-page app running inside Tauri's WebView, built with **Vite**, **TypeScript**, **Lit**, and vanilla CSS. It is deliberately framework-light: no router library, no state-management library, no component framework beyond Lit's rendering — the entire architecture fits in your head, and this page puts it there.

> **Where are the API docs?** There is no generated API reference for the frontend, on purpose. TypeDoc was assessed and skipped: this is an *application*, not a library — nothing imports it, so a generated reference would document internal wiring with no consumer. Instead, **every exported symbol in `frontend/src` carries a real TSDoc comment** (what it renders, the state it owns, the events it speaks, its keyboard contract), and this guide is the map that ties them together. Read this page first, then read the source — the comments are the reference.

**The ten-second tour:** `main.ts` boots [`App`](../../frontend/src/App.ts) into `#app`. App constructs the [`Router`](../../frontend/src/router.ts) (a 15-line reactive store) and mounts one of four views — **RecordingsView** (the library, home), **SettingsView**, **DoctorView**, or the **FirstRunWizard**. Views call the daemon through [`services/ipc.ts`](../../frontend/src/services/ipc.ts) and hear back through [`services/events.ts`](../../frontend/src/services/events.ts). That's the whole loop.

---

## 1. 🏗️ Architecture

### 1.1 Lit in the light DOM (`createRenderRoot() { return this; }`)

All Lit components override `createRenderRoot()` to render into the **light DOM** instead of a shadow root:

```typescript
import { LitElement, html } from "lit";
import { customElement } from "lit/decorators.js";

@customElement("ph-my-widget")
export class MyWidgetElement extends LitElement {
  protected createRenderRoot() {
    return this; // light DOM — global styles apply
  }

  render() {
    return html`<div class="mw-root">Hello</div>`;
  }
}
```

**Why:** the app is styled by global stylesheets — `styles/theme.css` (the CSS-variable themes), `styles/reset.css`, `styles/toast.css`, and the shared `components/modal.css` / `model-picker.css` / `tag-manager.css`. Shadow DOM boundaries would block all of them, forcing every component to re-import or re-declare its styles. Light DOM keeps one theme pipeline and lets idioms like `.modal-overlay` and `.settings-field` work everywhere.

**The cost, and the rule:** class names are global. Namespace them with a component prefix — `.hb-*` (HeaderBar), `.rv-*` (RecordingsView), `.mp-*` (ModelPicker), `.sv-*` (SettingsView), `.ov-*` (overlay), `.ss-*` (SavedSearches) — and you'll never collide.

Custom elements all use the `ph-` tag prefix (`ph-header-bar`, `ph-recordings-list`, `ph-tag-chips`, …).

### 1.2 Plain classes vs Lit components — the split, and when to use which

The codebase intentionally mixes two component styles:

| Style | Used for | Examples |
|---|---|---|
| **Plain TS class** (`constructor(container, …)` renders into the container) | Imperative orchestration: things that own *layout*, *lifecycle*, or *composition* rather than reactive templates | `App`, `RecordingsView`, `RecordingDetail`, `Splitter`, `NotesEditor`, the Settings section classes |
| **Lit component** (`@customElement`, reactive `@state`/`@property`) | Anything whose rendering follows data: lists, forms, chips, modals | `RecordingsListElement`, `HeaderBarElement`, `TagChipsElement`, `SettingsViewElement`, every modal |

Rules of thumb when adding something new:

- Rendering data that changes? **Lit component.** You get re-render-on-state for free.
- Wiring panes together, managing mount/dispose, owning drag/keyboard across children? **Plain class.** Lit buys nothing there.
- A plain class that needs a Lit child uses the **imperative mount wrapper** idiom — a thin class that `document.createElement("ph-…")`s the element, sets its properties, appends it, and re-exposes the element's API. `HeaderBar`, `RecordingsList`, `WaveformPlayer`, `TranscriptEditor`, `BulkActionBar`, `ActionRow`, `TagChips` are all wrappers of this shape (each is documented in its file).

One registration gotcha worth knowing before it bites you: if a module references a Lit component **only as a type**, the import is elided at build time and `@customElement` never runs — the tag renders as an inert unknown element. The fix is a bare side-effect import next to the type import; see the comment block above `import "./MergedConversationDetail"` in [`RecordingsView/index.ts`](../../frontend/src/components/RecordingsView/index.ts) — it documents the one place this regressed.

### 1.3 State: the `Store<T>` pattern

State management is one 60-line class, [`state/store.ts`](../../frontend/src/state/store.ts):

```typescript
export class Store<T> {
  get(): T;
  set(updater: T | ((prev: T) => T)): void;   // notifies subscribers
  subscribe(sub: (value: T) => void): () => void; // returns unsubscribe
}
```

Three things to internalize:

1. **Change detection is by identity (`===`).** Always set immutably: `store.set({ ...store.get(), field: x })`. Mutating the held object notifies nobody.
2. **`subscribe` fires immediately** with the current value, so a fresh subscriber renders without waiting for a change.
3. **You must call the returned unsubscribe function** on teardown (`disconnectedCallback` for Lit, `dispose()` for plain classes), or the store keeps your callback — and your component — alive.

Where the stores live:

| Store | Module | Holds |
|---|---|---|
| `filterStore` | [`state/filter.ts`](../../frontend/src/state/filter.ts) | The one shared library filter (`UiFilter`). Header search, sidebar, saved searches, and More-like-this all *write* it; `RecordingsList` *re-queries* on every change. |
| `router.state` | [`router.ts`](../../frontend/src/router.ts) | The active view name. `App.mount()` subscribes. |
| list state | created in `RecordingsView`, defined in [`RecordingsList.ts`](../../frontend/src/components/RecordingsView/RecordingsList.ts) | `{ recordings, selectedId, loading, error }`, shared by the view's panes. |

Smaller cross-cutting state uses module singletons instead of stores when nothing needs to *react* to it: [`state/openRecording.ts`](../../frontend/src/state/openRecording.ts) (which recording the detail pane shows — read by the header's "Run once" and the `phoneme:action` keyboard bridge) and [`state/savedSearches.ts`](../../frontend/src/state/savedSearches.ts) (localStorage-backed saved-search CRUD).

### 1.4 File layout map

```
frontend/src/
├── main.ts                 # entry: global CSS + new App(#app)
├── overlay.ts              # SEPARATE entry: the system-wide live-preview overlay window
├── App.ts                  # root controller: shell, router wiring, app-wide listeners
├── router.ts               # ViewName store ("recordings" | "settings" | "doctor" | "wizard")
├── components/
│   ├── HeaderBar.ts        # top bar: search/filters, record button, health pill
│   ├── ModelPicker.ts      # the unified Models modal (Save as default / Run once)
│   ├── ConfirmDelete.ts    # destructive-confirm dialog + confirmDelete()/confirmRecordingDelete()
│   ├── confirmDialog.ts    # generic themed yes/no modal
│   ├── DoctorModal.ts / DoctorView/   # health checks: quick modal + routed full page
│   ├── doctorChecks.ts     # shared Doctor logic (categories, Fix All plan, tallies)
│   ├── TagManager.ts       # quick modal shell around SectionTags (bare mode)
│   ├── SavedSearches.ts    # header 🔖 dropdown
│   ├── FirstRunWizard/     # guided setup (express + custom flows)
│   ├── RecordingsView/     # THE home view: index.ts (layout/keyboard/events) + panes
│   │   ├── RecordingsList.ts, RecordingDetail.ts, MergedConversationDetail.ts
│   │   ├── Sidebar.ts, QueuePanel.ts, FailedPanel.ts, BulkActionBar.ts
│   │   ├── ActionRow.ts, TagChips.ts, TranscriptEditor.ts, NotesEditor.ts
│   │   ├── WaveformPlayer.ts, TimelineView.ts, TranscriptDiff.ts, ThinkingPopout.ts
│   │   ├── grouping.ts, mergeMeeting.ts, rerunActions.ts   # pure logic, well-tested
│   │   └── Splitter.ts, RerunForm.ts, styles.css
│   ├── SettingsView/       # index.ts (tabs/search/save) + one Section* per tab
│   │   ├── form.ts         # renderField/bindFieldEvents: dotted-path config binding
│   │   ├── connectionField.ts, modelField.ts   # the shared provider/model controls
│   │   └── searchKeywords.ts                   # settings-search intent keywords
│   └── shared/settingsAnchor.ts                # ⚙ button position handoff
├── services/
│   ├── ipc.ts              # EVERY tauri command wrapper (typed)
│   ├── events.ts           # DaemonEvent types + subscribe()
│   ├── keyboard.ts         # global shortcuts, g-chords, vim layer, "?" help
│   ├── notifications.ts    # pipeline step/error toasts
│   ├── headerBar.ts        # top-bar show/hide animation
│   ├── recordStopMode.ts   # Record button stop behavior (persisted)
│   ├── llmProviders.ts / sttProviders.ts / llmModels.ts   # provider catalogs + model fetch
├── state/                  # store.ts, filter.ts, openRecording.ts, savedSearches.ts
├── data/curatedModels.ts   # shipped per-provider model recommendations
├── utils/                  # toast, error, format, date, diff, fuzzy, import, vimrc
└── styles/                 # theme.css (all themes), reset.css, toast.css, overlay.css
```

**The second window:** [`overlay.ts`](../../frontend/src/overlay.ts) is its own Vite entry (`overlay.html`, wired up in `vite.config.ts`'s `rollupOptions.input`) loaded by a separate Tauri `WebviewWindow` that the tray creates for the system-wide live-caption overlay. It is deliberately standalone — no App, no router — and listens to the same `daemon-event` stream. If you add a third window, mirror that pattern.

---

## 2. 🔄 Data flow

### 2.1 Commands: `services/ipc.ts` → Tauri → BridgeSlot → daemon

Every backend call goes through one typed wrapper in [`services/ipc.ts`](../../frontend/src/services/ipc.ts). The full path of, say, `listRecordings(filter)`:

```
ipc.ts listRecordings()
  → tauri invoke("list_recordings", { filter })       (WebView → tray process)
    → #[tauri::command] in src-tauri/src/commands.rs  (the tray)
      → BridgeSlot: the tray's named-pipe connection to the daemon
        → daemon ipc_handler → catalog (SQLite) → reply travels back up
```

Contract points to respect when adding a call (all documented at the top of `ipc.ts`):

- **Top-level argument keys are camelCase** — Tauri converts them to the command's snake_case parameters. But **nested object fields stay snake_case** (Tauri doesn't recurse), which is why `ListFilter` and `RerunAllOverrides` have snake_case fields. The `ipc.test.ts` suite pins exact command names and payloads for this reason.
- **Errors reject with `{ kind, message }`** (the structured `CommandError`). Normalize with [`utils/error.ts`](../../frontend/src/utils/error.ts)'s `errText(e)` / `errKind(e)`. The wrappers never toast — *callers* decide (UI components toast via `showToast`; pure logic re-throws).
- **Mutations don't return fresh state.** A successful `updateTranscript` resolves `void`; the new truth arrives as a `transcript_updated` daemon event. Don't hand-patch local state and call it done — refresh on the event (next section).
- **Secrets:** a saved API key arrives in config reads as the masked sentinel (`MASKED_SECRET` in [`services/llmModels.ts`](../../frontend/src/services/llmModels.ts), mirroring the tray). Never send the sentinel to a provider, and only write a key back if the user actually changed it.

### 2.2 Events: how the UI refreshes (event-driven, not polling)

[`services/events.ts`](../../frontend/src/services/events.ts) types the daemon's broadcast stream and exposes `subscribe(handler)`. The daemon emits over IPC; the tray re-emits each event as the Tauri event `"daemon-event"`; `DaemonEvent` mirrors the wire enum in `crates/phoneme-ipc/src/schema.rs` (add new events in **both** places).

```
daemon broadcast ──(pipe)──▶ tray bridge ──("daemon-event")──▶ subscribe() handlers
                                                                  │
                  RecordingsView.subscribeToEvents() ◀────────────┤  refresh list + open detail
                  QueuePanel (app-lifetime)          ◀────────────┤  queue rows, stage labels
                  notifications.ts (app-lifetime)    ◀────────────┤  step/error toasts
                  Sidebar, TagChips, ThinkingPopout… ◀────────────┘  tag reloads, AI activity log
```

The pattern: **events carry ids, not payloads** (mostly). `transcript_updated { id }` means "re-fetch recording `id` if you're showing it". `RecordingsView` owns the main subscription and refreshes the list + detail panes; always-mounted widgets (queue panel, step notifications) hold app-lifetime subscriptions that deliberately never unlisten. Everything else subscribes in `connectedCallback` and **must** unlisten in `disconnectedCallback`.

Two notable in-window event families complement the daemon stream (both plain `window` CustomEvents):

- **`config:saved`** — dispatched by SettingsView (and the Models modal, and profile switches) with the fresh config as `detail`. Listeners re-apply live: App (theme, titlebar), keyboard.ts (vim nav, animation speed, step-notification gate), RecordingsList (columns), SettingsView itself (re-mount sections). If your feature reads config at startup, also listen here so a Save applies without a reload.
- **`phoneme:*`** — the decoupling bus for cross-component actions, so deep components don't need callbacks threaded through: `phoneme:navigate` (deep links; App guards unsaved Settings edits), `phoneme:vim` + `phoneme:action` (keyboard layer → views, §3.1), `phoneme:request-delete` (any surface → RecordingsView's undoable-delete flow), `phoneme:select-recording`, `phoneme:open-split`, `phoneme:close-detail`, `phoneme:toggle-focus-mode`, `phoneme:enter-header-nav`, `phoneme:enter-bulk-bar`, `phoneme:sidebar-changed`, `phoneme:timeline-seek`/`-scroll` (synced dual timelines), and `phoneme:vim-save` (`:w` from the editors).

### 2.3 Toasts and step notifications

[`utils/toast.ts`](../../frontend/src/utils/toast.ts) is the singleton snackbar: `showToast(message, type, duration?)` (success 3s / info 3.5s / warning 6s / error 10s; `0` = sticky; hovering pauses the clock; stack capped at 6) and `showActionToast` (the Undo-delete flow: hide rows now, really delete on expiry, `onAction` cancels).

[`services/notifications.ts`](../../frontend/src/services/notifications.ts) turns pipeline events into toasts, with a **gating contract** worth knowing: step-completion toasts ("Transcribed ✓ — cleaning up…") are gated by `interface.step_notifications`, but **failure toasts always show** — silently losing a transcription is never acceptable. A user-initiated stage skip arrives as a `summary_failed` carrying the daemon's pinned skip sentinel and is reported as a skip, not an error (the regexes and the daemon constant are documented in the module).

---

## 3. ⌨️ UI systems

### 3.1 The keyboard layer

All global keys live in **one** document-level listener in [`services/keyboard.ts`](../../frontend/src/services/keyboard.ts). Its standing-down rules make the rest of the app simple:

- **Never while typing** (input/textarea/select/contentEditable) — except Esc from header inputs, which drops focus to the list.
- **Never while a modal is open** (`document.querySelector(".modal-overlay")` — see §3.2).
- **Never if a component already handled the key** (`e.defaultPrevented`).

What it dispatches (so where to look when a key "does something"):

| Keys | Mechanism |
|---|---|
| `/`, `?`, `Ctrl+,`, `Ctrl+/` | Direct: focus search, help overlay, Settings, hide top bar |
| `g` chords (`g l/s/d/D/T/P/S`, `g g`, `g /`) | A 1-second pending-`g` state, then `phoneme:navigate` or `phoneme:vim` |
| `p c e r` (open recording) | `phoneme:action` → the open recording's `ActionRow` (split panes check `getOpenRecordingId()` so only one acts) |
| `f`, `t`, `T` | Focus mode toggle / tag box / Tag Manager via window events |
| vim layer (`h j k l`, `gg G zz dd`, `i`, Enter) | `phoneme:vim` actions → **RecordingsView** performs them — keyboard.ts owns *gating and sequencing*, the view owns *the pane DOM* |

The vim layer (`interface.vim_nav`, off by default — every key below is inert when off) models the screen as a **2D grid of panes**: sidebar ⇄ list ⇄ detail (⇄ detail2 in split mode), `h`/`l` between panes, `j`/`k` within. Within the sidebar and detail panes, RecordingsView tracks row/column cursors over real DOM cells (`sidebarGrid()` / `DetailCell`s — buttons, the tag box, the editors) and paints the cursor with the `.kbd-cursor` class. The header is its own strip: `k` at the top of the list highlights (not focuses) the search box, `h`/`l` roam the header controls, Enter sub-navigates dropdowns — all documented in `keyboard.ts`.

The list keeps its own arrow/Enter/Space navigation in `RecordingsList` (it works without vim nav); the transcript/notes editors have their *own* vim mode (`editor.vim_mode`, CodeMirror + `@replit/codemirror-vim`, `:w`/`:wq`/`:q` via [`utils/vimrc.ts`](../../frontend/src/utils/vimrc.ts)) — the global layer never steals keys from a focused editor.

**Adding a shortcut:** put the binding in `onKeyDown` (or dispatch a `phoneme:vim` action and handle it in `RecordingsView.handleVim`), and **always** add it to `BASE_HELP_GROUPS` / `VIM_HELP_GROUP` — the `?` cheat-sheet renders from that registry, and an undocumented shortcut is treated as a bug. Then document it in `docs/user-guide/keyboard_navigation.md`.

### 3.2 Modal & overlay idioms

One idiom, used by every dialog (`ConfirmDelete`, `confirmDialog`, `DoctorModal`, `ModelPicker`, `TagManager`, `FailedPanel`, the `?` help, the bulk bar's modals):

1. Render a `.modal-overlay` wrapping a `.modal-dialog` (shared styles in `components/modal.css`).
2. The overlay's *presence* is the contract: both global keyboard layers stand down while one exists, so **your modal owns Escape** — close on it (and on overlay click).
3. Self-removing promise wrapper: `document.createElement("ph-…")`, append to `<body>`, await a `resolved` CustomEvent, `el.remove()`, resolve the detail. See `confirmRecordingDelete()` or `openModelPicker()` for the canonical shape.

Components that must beat the global layers to a key (e.g. the header's dropdown Escape, the bulk bar's nav) use **capture-phase listeners + `stopPropagation`** — grep `addEventListener(".."., true)` for examples; each one carries a comment explaining what it must not fall through to.

### 3.3 The shared connection & model fields

Every provider/model picker in the app — Settings (Transcription, Post-Processing, Summary, Auto-Tag, Live Preview, Dictation), the Models modal, and (via the catalogs) the wizard and Re-run form — is built from two vanilla-DOM controls in `components/SettingsView/`:

- [`connectionField.ts`](../../frontend/src/components/SettingsView/connectionField.ts) — `mountConnectionField(host, opts)`: a grouped select of **named** providers ("On this computer" / "Cloud" / "Advanced"), a key row shown only when needed, a Test button, and the endpoint URL under an Advanced disclosure. It reads/writes the existing config shape (wire `provider` kind + `api_url`) through your getters/setters, and derives the displayed selection back from `(kind, api_url)` — saved configs round-trip with zero migration.
- [`modelField.ts`](../../frontend/src/components/SettingsView/modelField.ts) — `mountModelField(host, opts)`: a model dropdown seeded from the shipped curated catalog ([`data/curatedModels.ts`](../../frontend/src/data/curatedModels.ts)) for the *currently selected* provider, live-merged with the provider's `/models` listing in LLM mode, plus ↻ Refresh and an "Other… (type)" free-text fallback.

Minimal usage (the shape every section follows — getters read the live config object, setters write it):

```typescript
const llm = config.llm_post_process;
mountConnectionField(container.querySelector("#conn-host")!, {
  catalog: "llm",
  getKind: () => llm.provider, setKind: (k) => { llm.provider = k; },
  getApiUrl: () => llm.api_url, setApiUrl: (u) => { llm.api_url = u; },
  getApiKey: () => llm.api_key, setApiKey: (k) => { llm.api_key = k; },
  onProviderChanged: () => remountModelField(), // model suggestions follow the provider
});
mountModelField(container.querySelector("#model-host")!, {
  mode: "llm",
  getProvider: () => llm.provider,
  getApiUrl: () => llm.api_url, getApiKey: () => llm.api_key,
  getModel: () => llm.model, setModel: (m) => { llm.model = m; },
});
```

Provider *catalogs* live in [`services/llmProviders.ts`](../../frontend/src/services/llmProviders.ts) (LLM presets: friendly name → one of four wire protocols + default endpoint/model) and [`services/sttProviders.ts`](../../frontend/src/services/sttProviders.ts) (STT). **Adding a provider to the catalog makes it appear everywhere the shared picker is used** — that's the point. The `inheritLabel` option renders the "Same as Post-Processing" anchor that Summary/Auto-Tag/Title use (blank provider = inherit the cleanup connection, mirroring the daemon's fallback).

### 3.4 Theming

Themes are pure CSS variables: [`styles/theme.css`](../../frontend/src/styles/theme.css) defines `:root` (Catppuccin Mocha, the default) plus one `html[data-theme="…"]` block per theme, each setting the same ~15 variables (`--bg-deep`, `--bg-surface`, `--bg-elevated`, `--fg-default`, `--fg-muted`, `--fg-faded`, `--accent`, `--border-subtle`, `--popup-border`, status colors…). App sets `data-theme` from `interface.theme` at startup and on every `config:saved`; the overlay window applies it independently.

House idioms:

- **Components never hardcode colors** — always `var(--…)`. New CSS that needs a tint derives it with **`color-mix`**, e.g. the pill/hover idiom: `background: color-mix(in srgb, var(--accent) 15%, transparent)`. This is everywhere (≈90 uses in RecordingsView/styles.css alone) and is what keeps every theme working without per-theme rules. Tag colors are the one user-chosen exception — `getContrastColor()` in `TagChips.ts` picks readable text for them.
- **Animation speed** is the `--pane-anim` duration variable on `<html>`, set by keyboard.ts from `interface.animation_speed` (`off` 0ms / `fast` 110ms / `normal` 200ms / `slow` 320ms). Pane slides, the header collapse ([`services/headerBar.ts`](../../frontend/src/services/headerBar.ts)), and layout transitions all read it — use it for any new layout animation so "off" really means off.

### 3.5 Persisted UI preferences (every localStorage key)

Per-device UI state lives in localStorage, **never** in config.toml (config is for behavior; these are window-layout memories). All keys share the `phoneme` prefix, and Settings → Interface → "Reset interface preferences" clears everything with that prefix. Wrap reads/writes in `try/catch` (private mode) and prefer a `LS_*` const next to the consumer, with a doc comment.

| Key | Owner | Stores |
|---|---|---|
| `phoneme.layout.splitPercent` | RecordingsView | List↔detail split, % (20–80, default 61) |
| `phoneme.layout.splitRatio` | RecordingsView | Split-mode pane↔pane ratio, % (20–80, default 50) |
| `phoneme.layout.sidebarOpen` | RecordingsView | Sidebar visibility (default open) |
| `phoneme.layout.sidebarWidth` | RecordingsView | Sidebar width, px (160–480, default 200) |
| `phoneme.layout.selectedId` | RecordingsView | Last-selected recording (or `session:<meeting_id>`), restored on reload |
| `phoneme.layout.listZoom` | RecordingsView | List zoom factor (0.6–2; Ctrl+scroll / Ctrl+= − 0) |
| `phoneme.layout.headerHidden` | services/keyboard.ts | Ctrl+/ top-bar hidden flag |
| `phoneme.recordMode` | HeaderBar | Record button mode: `recording` \| `meeting` |
| `phoneme.recordStopMode` / `phoneme.recordStopDurationSecs` | services/recordStopMode.ts | Stop behavior (`toggle`/`silence`/`duration`) + fixed length, seconds |
| `phoneme.semanticSearch` | HeaderBar (+ saved-search apply) | ✨ semantic-search default for the search box |
| `phoneme.savedSearches` | state/savedSearches.ts | The saved-search list (JSON array of filter snapshots) |
| `phoneme.expandedMeetings` | RecordingsList | Which meeting groups are expanded (JSON string array) |
| `phoneme.meetingIcons` | RecordingsList | Per-meeting emoji, `{ meetingId: icon }` |
| `phoneme.sidebar.libraryOpen` / `phoneme.sidebar.tagsOpen` | Sidebar | Section fold state |
| `phoneme.queuePanelCollapsed` / `phoneme.queueListHeight` | QueuePanel | Queue panel fold + dragged list height (px) |
| `phoneme.bulkBarPos` | BulkActionBar | Dragged floating position (JSON `{x,y}`) |
| `phoneme.thinkingFabPos` / `phoneme.thinkingFabOpen` / `phoneme.thinkingPanelGeom` | ThinkingPopout | 🧠 AI-activity button position, open state, panel geometry |
| `phoneme.activeProfile` | SectionProfiles | Name of the last-applied config profile |
| `phoneme_skip_delete_confirm` + `phoneme_delete_mode` | ConfirmDelete | "Don't ask again" for recording deletes + the pinned delete mode (`everything`/`keep_audio`) |
| `phoneme_skip_tag_delete_confirm` | SectionTags | "Don't ask again" for tag deletes |
| `phoneme_skip_profile_update_confirm` / `phoneme_skip_profile_delete_confirm` | SectionProfiles | "Don't ask again" for profile overwrite/delete |

(The underscore-style `phoneme_*` keys predate the dotted convention; new keys use dots. Both are cleared by the reset button.)

---

## 4. 🧪 Testing

### 4.1 Setup

Vitest with the **jsdom** environment (set in [`vite.config.ts`](../../frontend/vite.config.ts) — component tests touch `document`/`window`). Tests are colocated: `foo.ts` → `foo.test.ts`, currently 31 files / 374 tests covering services, state, utils, pure view logic (grouping, merging, diff), and component behavior (modals, forms, panels).

```powershell
cd frontend
npx vitest run                 # full suite (CI runs this)
npx vitest run src/services/ipc.test.ts   # one file
npx vitest                     # watch mode
npx tsc --noEmit               # type-check
pnpm lint                      # eslint (zero errors expected)
```

CI (`.github/workflows/ci.yml`, `frontend` job, Node 20 + pnpm) gates every push on **lint → vitest → type-check**. All three must be clean locally before you call a change done.

### 4.2 House mocking idioms

Real examples to copy (each named test file demonstrates the idiom):

- **Module-mock the Tauri boundary** (`ipc.test.ts`): `vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))`, then assert *exact command names and payload shapes* — `expect(invoke).toHaveBeenCalledWith('list_meeting', { meetingId: 'sess-1' })`. These tests pin the wire contract (camelCase top-level keys, snake_case nested), so a renamed command fails loudly.
- **The event-capture trick** (`events.test.ts`, `notifications.test.ts`): mock `subscribe`/`listen` to *capture the registered handler* into a local, then drive synthetic `DaemonEvent`s straight through it (`emit({ event: "pipeline_stage_changed", … })`). No timers, no real event plumbing.
- **Partial mocks keep the real contract** (`notifications.test.ts`): `vi.importActual` the module and override only `subscribe`, so `stageLabel`'s real wording is what the toast assertions pin — the test fails if user-facing text drifts.
- **Spy on collaborators, not the DOM** (`notifications.test.ts` mocks `../utils/toast`; rendering itself is covered once, in `toast.test.ts`).
- **Component tests mount for real** (`ConfirmDelete.test.ts`, `SettingsView/index.test.ts`): create the element, append to `document.body`, click buttons, assert dispatched events and localStorage effects; reset storage in `beforeEach`.
- **Pure logic gets plain tests** (`grouping.test.ts`, `mergeMeeting.test.ts`, `diff.test.ts`, `filter.test.ts`): no mocks at all — which is itself the idiom: extract decision logic into pure modules so most tests need no environment.

When adding a feature, mirror the nearest test file's approach. New IPC wrappers get a payload-pinning test; new event consumers get a captured-handler test; new pure helpers get direct tests.

---

## 5. 🧭 Where do I change X? (cookbook)

| I want to… | Touch |
|---|---|
| **Add a Tauri/IPC call** | Command in `src-tauri/src/commands.rs` (and the daemon request in `crates/phoneme-ipc/src/schema.rs` + `bin/phoneme-daemon/src/ipc_handler.rs` if it reaches the daemon) → typed wrapper in `services/ipc.ts` → payload-pinning test in `ipc.test.ts` → document in `docs/developer-guide/ipc_integration.md` |
| **React to a new daemon event** | Add the variant to `DaemonEvent` in `services/events.ts` (mirroring `schema.rs`) → handle it where it matters (usually `RecordingsView.subscribeToEvents`, `QueuePanel`, or `notifications.ts`) |
| **Add a setting** | The serde field in `crates/phoneme-core/src/config.rs` → the right `SettingsView/Section*.ts` (a `renderField` row bound to its dotted path; seed the table if it's new) → intent keywords in `searchKeywords.ts` if the label isn't obvious → `docs/developer-guide/config_reference.md` + the relevant user-guide page |
| **Add a Settings tab** | New `Section*.ts` class → register it in `SettingsView/index.ts` (`mountSection` + the tab rail) |
| **Add a recordings-list column** | `COLUMN_CATALOG` + defaults in `SectionInterface.ts` → render it in `RecordingsList.ts` (header + row + width handling) |
| **Add a keyboard shortcut** | `services/keyboard.ts` (`onKeyDown` + the `BASE_HELP_GROUPS`/`VIM_HELP_GROUP` registry) → if it acts on panes, a `phoneme:vim` action handled in `RecordingsView.handleVim` → `docs/user-guide/keyboard_navigation.md` |
| **Add an action on the open recording** | `ActionRow.ts` (button + `phoneme:action` case) → bind a key in `keyboard.ts` if it deserves one |
| **Add a bulk action** | `BulkActionBar.ts` (button + handler over `selected`) |
| **Add an LLM/STT provider** | `services/llmProviders.ts` or `services/sttProviders.ts` (catalog entry) → curated models in `data/curatedModels.ts` → it appears in every shared picker; `docs/user-guide/providers_and_models.md` |
| **Add a modal** | Follow §3.2: `.modal-overlay` idiom + self-removing promise wrapper (copy `confirmDialog.ts`) |
| **Add a theme** | One `html[data-theme="…"]` block in `styles/theme.css` (set every variable) → the theme `<select>` options in `SectionInterface.ts` |
| **Persist a UI preference** | A `phoneme.`-prefixed localStorage key next to its consumer (try/catch, documented const) → add it to the table in §3.5 |
| **Change toast behavior** | `utils/toast.ts` (mechanics) vs `services/notifications.ts` (which pipeline events toast, gating) |
| **Touch the live-preview overlay** | `overlay.ts` + `styles/overlay.css` (window creation lives in `src-tauri/src/overlay.rs`) |
| **Change Doctor checks** | Backend checks come from the tray/daemon; shared GUI logic (categories, Fix All, badges) in `components/doctorChecks.ts`; surfaces in `DoctorModal.ts` / `DoctorView/` |

---

*Related pages: [Architecture](architecture.md) · [Backend Guide](backend_guide.md) · [IPC Integration](ipc_integration.md) · [Config Reference](config_reference.md) · [Testing & CI](testing_and_ci.md) · [How to Extend](how_to_extend.md)*
