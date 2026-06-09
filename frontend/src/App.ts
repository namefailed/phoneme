import { HeaderBar } from "./components/HeaderBar";
import { RecordingsView } from "./components/RecordingsView";
import { SettingsView } from "./components/SettingsView";
import { DoctorView } from "./components/DoctorView";
import { FirstRunWizard } from "./components/FirstRunWizard";
import { Router, type ViewName } from "./router";
import { onNav } from "./services/events";
import { initKeyboard } from "./services/keyboard";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { importAudioPaths } from "./utils/import";

/// A mounted view. Every view exposes an optional `dispose` for teardown
/// (RecordingsView unsubscribes its event listeners there).
type MountedView = { dispose?: () => void };

/**
 * The root Application controller.
 * Responsible for initializing the main shell, the routing layer, and bootstrapping
 * initial states like theming and first-run wizard checks.
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
          if (this.current.canClose()) {
            this.router.go("recordings");
          }
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
    void onNav("settings", () => this.router.go("settings"));
    void onNav("doctor", () => this.router.go("doctor"));

    // In-app navigation shortcuts (decoupled window event so deep components
    // don't need a routing callback threaded through). e.g. the Re-run menu's
    // "Enable cleanup in Settings" jumps straight to the Post-Processing tab.
    window.addEventListener("phoneme:navigate", (e) => {
      const detail = (e as CustomEvent).detail ?? {};
      if (detail.view === "settings") {
        this.pendingSettingsTab = typeof detail.section === "string" ? detail.section : null;
        this.router.go("settings");
      } else if (detail.view === "recordings") {
        this.router.go("recordings");
      } else if (detail.view === "doctor") {
        this.router.go("doctor");
      }
    });

    // Global keyboard shortcuts (focus search, navigate, "?" cheat-sheet).
    initKeyboard();

    // Tray menu recording commands.
    void listen("menu:record", async () => {
      await invoke("record_start", { mode: "oneshot" });
    });
    void listen("menu:stop", async () => {
      await invoke("record_stop");
    });

    // Auto-launch the first-run wizard if no config exists yet.
    void this.maybeAutoWizard();
    void this.loadAndApplyTheme();
    void this.setupFileDrop();

    window.addEventListener("config:saved", (e: any) => {
      const cfg = e.detail;
      if (cfg?.interface?.theme) {
        document.documentElement.setAttribute("data-theme", cfg.interface.theme);
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

  private mount(view: ViewName) {
    this.current?.dispose?.();
    this.mainEl.innerHTML = "";
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
  }
}
