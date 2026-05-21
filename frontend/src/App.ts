import { HeaderBar } from "./components/HeaderBar";
import { RecordingsView } from "./components/RecordingsView";

export class App {
  private container: HTMLElement;
  private header: HeaderBar;
  private recordings: RecordingsView;

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

    this.header = new HeaderBar(this.container.querySelector("#hdr") as HTMLElement, {
      onSearchChange: () => {
        // Search wiring lands in Plan 5 (Settings + filtering).
      },
      onOpenSettings: () => {
        // Settings view lands in Plan 5.
      },
    });

    this.recordings = new RecordingsView(this.container.querySelector("#main") as HTMLElement);
  }

  dispose() {
    this.recordings.dispose();
    void this.header;
  }
}
