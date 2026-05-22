// RecordingsView — the home view's split layout, live updates, keyboard.

import { subscribe, type DaemonEvent } from "../../services/events";
import { Store } from "../../state/store";
import { RecordingsList, type RecordingsListState } from "./RecordingsList";
import { RecordingDetail } from "./RecordingDetail";
import { Splitter } from "./Splitter";
import "./styles.css";

export class RecordingsView {
  private container: HTMLElement;
  private list: RecordingsList;
  private detail: RecordingDetail;
  private state: Store<RecordingsListState>;
  private splitPercent = 50;
  private detailVisible = true;
  private unsub: (() => void) | null = null;

  constructor(container: HTMLElement) {
    this.container = container;
    this.state = new Store<RecordingsListState>({
      recordings: [],
      selectedId: null,
      loading: false,
      error: null,
    });

    this.container.innerHTML = `
      <div class="rv-shell" id="rv-shell">
        <div class="rv-list" id="rv-list"></div>
        <div class="rv-splitter" id="rv-split"></div>
        <div class="rv-detail" id="rv-detail"></div>
      </div>
    `;

    const listRoot = this.container.querySelector<HTMLElement>("#rv-list")!;
    const detailRoot = this.container.querySelector<HTMLElement>("#rv-detail")!;
    const splitRoot = this.container.querySelector<HTMLElement>("#rv-split")!;

    this.list = new RecordingsList(listRoot, this.state, (id) => this.onSelect(id));
    this.detail = new RecordingDetail(detailRoot, () => {
      void this.refresh();
    });
    new Splitter(splitRoot, this.splitPercent, (pct) => {
      this.splitPercent = pct;
      this.applyLayout();
    });

    this.applyLayout();
    void this.refresh();
    void this.subscribeToEvents();
    this.installShortcuts();
  }

  async refresh() {
    await this.list.refresh();
    const s = this.state.get();
    if (s.selectedId && !s.recordings.some(r => r.id === s.selectedId)) {
      this.state.set({ ...s, selectedId: null });
      this.detail.clear();
    }
  }

  toggleDetail() {
    this.detailVisible = !this.detailVisible;
    this.applyLayout();
  }

  dispose() {
    if (this.unsub) {
      this.unsub();
      this.unsub = null;
    }
  }

  private applyLayout() {
    const shell = this.container.querySelector<HTMLElement>("#rv-shell");
    if (!shell) return;
    if (this.detailVisible) {
      shell.style.gridTemplateColumns = `${this.splitPercent}% 4px ${100 - this.splitPercent}%`;
    } else {
      shell.style.gridTemplateColumns = `1fr 0 0`;
    }
  }

  private onSelect(id: string) {
    this.state.set({ ...this.state.get(), selectedId: id });
    void this.detail.show(id);
  }

  private async subscribeToEvents() {
    this.unsub = await subscribe((event: DaemonEvent) => {
      const eventName = (event as { event: string }).event;
      if (
        eventName === "recording_stopped" ||
        eventName === "transcription_done" ||
        eventName === "transcription_failed" ||
        eventName === "hook_done" ||
        eventName === "hook_failed" ||
        eventName === "recording_deleted" ||
        eventName === "transcript_updated"
      ) {
        void this.refresh();
      }
    });
  }

  private installShortcuts() {
    document.addEventListener("keydown", async (e) => {
      // Ignore keydown if we are inside an input/textarea
      const target = e.target as HTMLElement;
      if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;

      if (e.ctrlKey && e.key === "\\") {
        e.preventDefault();
        this.toggleDetail();
      } else if (e.key === "Delete") {
        const id = this.state.get().selectedId;
        if (id) {
          e.preventDefault();
          const { confirmDelete } = await import("../ConfirmDelete");
          if (await confirmDelete()) {
            const { deleteRecording } = await import("../../services/ipc");
            await deleteRecording(id, false);
            this.refresh();
          }
        }
      }
    });
  }
}
