import { HeaderBar } from "./components/HeaderBar";
import { RecordingsView } from "./components/RecordingsView";
import { SettingsView } from "./components/SettingsView";
import { DoctorView } from "./components/DoctorView";
import { FirstRunWizard } from "./components/FirstRunWizard";
import { Router, type ViewName } from "./router";
import { onNav } from "./services/events";
import { initKeyboard } from "./services/keyboard";
import { initCursorAnimation } from "./services/cursorAnimation";
import { initSmearCaret } from "./services/smearCaret";
import { initStepNotifications } from "./services/notifications";
import { setSettingsAnchor } from "./components/shared/settingsAnchor";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { importAudioPaths } from "./utils/import";

/// A mounted view. Every view exposes an optional `dispose` for teardown
/// (RecordingsView unsubscribes its event listeners there).
type MountedView = { dispose?: () => void };

/**
 * The root Application controller — the only object `main.ts` creates.
 * Builds the shell (header slot + main slot), constructs the Router and
 * mounts/disposes the active view on its changes, and wires every app-wide
 * concern once: the global keyboard layer, pipeline toast notifications,
 * tray-menu events (`menu:*` / `nav:*`), the `phoneme:navigate` window event
 * (in-app deep links, routed through the Settings unsaved-edits guard),
 * `config:saved` re-theming, native file drag-drop import, and the
 * first-run-wizard auto-launch when no config exists.
 *
 * Plain class, not a Lit component: it owns imperative lifecycle (mount /
 * dispose of whole views) rather than reactive rendering.
 */
export class App {
  private container: HTMLElement;
  private router = new Router();
  private mainEl: HTMLElement;
  private headerEl: HTMLElement;
  private current: MountedView | null = null;
  // When an in-app shortcut (e.g. Re-run → "Enable cleanup in Settings") routes
  // to Settings, the tab it wants opened. Consumed on the next settings mount.
  private pendingSettingsTab: string | null = null;

  constructor(container: HTMLElement) {
    this.container = container;
    this.container.innerHTML = `
      <div class="app-shell">
        <div id="hdr"></div>
        <div id="main"></div>
      </div>
    `;
    this.headerEl = this.container.querySelector("#hdr") as HTMLElement;
    this.mainEl = this.container.querySelector("#main") as HTMLElement;

    new HeaderBar(this.headerEl, {
      onOpenSettings: () => {
        if (this.current instanceof SettingsView) {
          void this.tryNavigate("recordings");
        } else {
          this.router.go("settings");
        }
      },
      onToggleSidebar: () => {
        if (this.current instanceof RecordingsView) {
          this.current.toggleSidebar();
        }
      },
    });

    this.router.state.subscribe((s) => this.mount(s.current));

    // Tray menu navigation.
    void onNav("settings", () => void this.tryNavigate("settings"));
    void onNav("doctor", () => void this.tryNavigate("doctor"));

    // In-app navigation shortcuts (decoupled window event so deep components
    // don't need a routing callback threaded through). e.g. the Re-run menu's
    // "Enable cleanup in Settings" jumps straight to the Post-Processing tab.
    window.addEventListener("phoneme:navigate", (e) => {
      const detail = (e as CustomEvent).detail ?? {};
      const tab = typeof detail.section === "string" ? detail.section : null;
      if (detail.view === "settings") void this.tryNavigate("settings", tab);
      else if (detail.view === "recordings") void this.tryNavigate("recordings");
      else if (detail.view === "doctor") void this.tryNavigate("doctor");
    });

    // Global keyboard shortcuts (focus search, navigate, "?" cheat-sheet).
    initKeyboard();
    // Optional cursor-move animation for the roving keyboard cursor (opt-in,
    // Settings → Appearance; honors prefers-reduced-motion).
    initCursorAnimation();
    // Smear caret for the CodeMirror editors — same cursor_animation setting.
    initSmearCaret();
    // Pipeline progress + error toasts (lifetime subscription).
    void initStepNotifications();

    // Tray menu recording commands. Catch invoke failures so a backend error
    // surfaces in the console instead of becoming an unhandled promise rejection.
    void listen("menu:record", async () => {
      try {
        await invoke("record_start", { mode: "oneshot" });
      } catch (e) {
        console.error("menu:record — record_start failed:", e);
      }
    });
    void listen("menu:stop", async () => {
      try {
        await invoke("record_stop");
      } catch (e) {
        console.error("menu:stop — record_stop failed:", e);
      }
    });

    // Auto-launch the first-run wizard if no config exists yet.
    void this.maybeAutoWizard();
    void this.loadAndApplyTheme();
    void this.setupFileDrop();

    window.addEventListener("config:saved", (e: any) => {
      const cfg = e.detail;
      if (cfg?.interface?.theme) {
        // Cross-fade the whole UI's colours on a runtime theme switch. The
        // `theme-anim` class is only on for the swap, so it never slows ordinary
        // hovers; gated by --ui-motion (instant when motion is off / reduced).
        const root = document.documentElement;
        root.classList.add("theme-anim");
        root.setAttribute("data-theme", cfg.interface.theme);
        window.setTimeout(() => root.classList.remove("theme-anim"), 380);
      }
      if (cfg?.interface?.strip_titlebar !== undefined) {
        getCurrentWindow().setDecorations(!cfg.interface.strip_titlebar);
      }
    });
  }

  private async loadAndApplyTheme() {
    try {
      const cfg = await invoke<any>("read_config");
      if (cfg?.interface?.theme) {
        document.documentElement.setAttribute("data-theme", cfg.interface.theme);
      }
      if (cfg?.interface?.strip_titlebar) {
        getCurrentWindow().setDecorations(false);
      } else {
        getCurrentWindow().setDecorations(true);
      }
    } catch (e) {
      console.warn("Failed to load or apply theme:", e);
      // fallback to default theme defined in CSS
    }
  }

  /**
   * Import audio files dropped onto the app window. Uses Tauri's native
   * drag-drop event (paths are real filesystem paths, which the daemon needs —
   * the browser File API would only give us opaque blobs).
   */
  private async setupFileDrop() {
    try {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      await getCurrentWebview().onDragDropEvent((event) => {
        if (event.payload.type === "drop") {
          const paths = event.payload.paths ?? [];
          if (paths.length > 0) {
            void importAudioPaths(paths);
          }
        }
      });
    } catch (e) {
      console.warn("Failed to register file-drop handler:", e);
    }
  }

  private async maybeAutoWizard() {
    try {
      const exists = await invoke<boolean>("config_exists");
      if (!exists) {
        this.router.go("wizard");
      }
    } catch (e) {
      console.error("Failed to check if config exists. Backend may be unreachable:", e);
      // If the backend isn't reachable, stay on the default view.
    }
  }

  /**
   * Navigate to `view`, but if we're currently in Settings with unsaved edits,
   * ask first (themed prompt). EVERY leave-path — the Settings button, the
   * quick-menu, `g`-nav, and the tray menu — funnels through here, so unsaved
   * changes can't be lost silently (the bare `router.go` only guarded the
   * Settings button before). `settingsTab` is the tab to open when entering
   * Settings; it's applied only once the navigation is allowed to proceed.
   */
  private async tryNavigate(view: ViewName, settingsTab: string | null = null) {
    if (this.current instanceof SettingsView && !(await this.current.confirmClose())) {
      return; // user chose to keep editing
    }
    if (view === "settings") this.pendingSettingsTab = settingsTab;
    this.router.go(view);
  }

  private mount(view: ViewName) {
    this.current?.dispose?.();
    this.mainEl.innerHTML = "";
    // Capture the header ⚙ Settings button's exact position BEFORE hiding the
    // header, so the Settings view can place its floating ⚙ button on the same
    // spot. Done here (not only in the header click handler) so it works no
    // matter how Settings was opened — header button, Ctrl+,, tray, deep link.
    if (view === "settings") {
      const btn = document.querySelector<HTMLElement>(".hb-settings-main");
      if (btn) {
        const r = btn.getBoundingClientRect();
        // Round to whole pixels: the float button (and the health pill/chevron it
        // carries) is positioned at these coords, and a fractional left/top makes
        // the thin-stroked caret straddle the pixel grid and render a hair soft —
        // looking very slightly smaller than the header's grid-aligned one. The
        // sub-pixel shift is invisible (the header is hidden while Settings shows).
        setSettingsAnchor({
          top: Math.round(r.top),
          left: Math.round(r.left),
          width: Math.round(r.width),
          height: Math.round(r.height),
        });
      }
    }
    // The top header bar is useless in Settings / the setup wizard — hide it
    // instantly and completely (there's nothing to slide it back into). Focus
    // mode / list zen / Ctrl+/ use the separate `phoneme-hide-header` class,
    // which ANIMATES the bar's collapse instead of removing it outright.
    document.body.classList.toggle("phoneme-hide-header-instant", view === "settings" || view === "wizard");
    switch (view) {
      case "recordings":
        this.current = new RecordingsView(this.mainEl);
        break;
      case "settings": {
        const initialTab = this.pendingSettingsTab;
        this.pendingSettingsTab = null;
        this.current = new SettingsView(this.mainEl, () => this.router.go("recordings"), () => this.router.go("wizard"), initialTab);
        break;
      }
      case "doctor":
        this.current = new DoctorView(this.mainEl, () => this.router.go("recordings"));
        break;
      case "wizard":
        this.current = new FirstRunWizard(this.mainEl, () => this.router.go("recordings"));
        break;
    }
    // Cross-fade the freshly-mounted view in. Remove → reflow → re-add restarts
    // the keyframe on every navigation. Gated by --ui-motion (shared/styles.css),
    // so it's instant when motion is off / reduced.
    this.mainEl.classList.remove("ph-view-enter");
    void this.mainEl.offsetWidth;
    this.mainEl.classList.add("ph-view-enter");
  }
}
