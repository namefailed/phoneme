import { HeaderBar } from "./components/HeaderBar";
import { RecordingsView } from "./components/RecordingsView";
import { SettingsView } from "./components/SettingsView";
import { DoctorView } from "./components/DoctorView";
import { FirstRunWizard } from "./components/FirstRunWizard";
import { Router, type ViewName } from "./router";
import { onNav } from "./services/events";
import { invoke } from "@tauri-apps/api/core";

/// A mounted view. Every view exposes an optional `dispose` for teardown
/// (RecordingsView unsubscribes its event listeners there).
type MountedView = { dispose?: () => void };

export class App {
  private container: HTMLElement;
  private router = new Router();
  private mainEl: HTMLElement;
  private headerEl: HTMLElement;
  private current: MountedView | null = null;

  constructor(container: HTMLElement) {
    this.container = container;
    this.container.innerHTML = `
      <div class="app-shell">
        <div id="hdr"></div>
        <div id="main"></div>
      </div>
      <style>
        .app-shell { display: grid; grid-template-rows: auto 1fr; height: 100vh; }
        #main { overflow: hidden; }
        .rv-shell {
          display: grid;
          height: 100%;
        }
        .rv-list, .rv-detail { overflow: auto; }
        .rv-splitter { background: var(--border); cursor: col-resize; }
        .rv-splitter:hover { background: var(--accent); }
      </style>
    `;
    this.headerEl = this.container.querySelector("#hdr") as HTMLElement;
    this.mainEl = this.container.querySelector("#main") as HTMLElement;

    new HeaderBar(this.headerEl, {
      onSearchChange: () => {},
      onOpenSettings: () => this.router.go("settings"),
    });

    this.router.state.subscribe((s) => this.mount(s.current));

    // Tray menu navigation.
    void onNav("settings", () => this.router.go("settings"));
    void onNav("doctor", () => this.router.go("doctor"));

    // Auto-launch the first-run wizard if no config exists yet.
    void this.maybeAutoWizard();
  }

  private async maybeAutoWizard() {
    try {
      const exists = await invoke<boolean>("config_exists");
      if (!exists) {
        this.router.go("wizard");
      }
    } catch {
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
      case "settings":
        this.current = new SettingsView(this.mainEl, () => this.router.go("recordings"));
        break;
      case "doctor":
        this.current = new DoctorView(this.mainEl, () => this.router.go("recordings"));
        break;
      case "wizard":
        this.current = new FirstRunWizard(this.mainEl, () => this.router.go("recordings"));
        break;
    }
  }
}
