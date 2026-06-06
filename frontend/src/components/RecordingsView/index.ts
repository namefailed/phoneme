// RecordingsView — the home view's split layout, live updates, keyboard.

import { subscribe, type DaemonEvent } from "../../services/events";
import { Store } from "../../state/store";
import { RecordingsList, type RecordingsListState } from "./RecordingsList";
import { RecordingDetail } from "./RecordingDetail";
import { MergedConversationDetail } from "./MergedConversationDetail";
import { BulkActionBar } from "./BulkActionBar";
import { Splitter } from "./Splitter";
import "./styles.css";

export class RecordingsView {
  private container: HTMLElement;
  private list: RecordingsList;
  private detail: RecordingDetail;
  private mergedDetail: MergedConversationDetail;
  private state: Store<RecordingsListState>;
  private splitPercent = 50;
  private detailVisible = true;
  private unsub: (() => void) | null = null;
  private keydownHandler: (e: KeyboardEvent) => void;

  /** Current multi-selection. Empty when no checkboxes are checked. */
  private multiSelected = new Set<string>();
  /** Reference to the bulk bar root element for re-mounting. */
  private bulkBarRoot: HTMLElement | null = null;

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
        <div class="rv-list" id="rv-list">
          <div id="rv-list-inner" style="height:100%; overflow:hidden;"></div>
          <div id="rv-bulk-bar" style="display:none;"></div>
        </div>
        <div class="rv-splitter" id="rv-split"></div>
        <div class="rv-detail" id="rv-detail">
          <div id="rv-single-detail" style="height: 100%;"></div>
          <ph-merged-conversation-detail id="rv-merged-detail" style="display:none; height: 100%;"></ph-merged-conversation-detail>
        </div>
      </div>
    `;

    const listRoot = this.container.querySelector<HTMLElement>("#rv-list-inner")!;
    const detailRoot = this.container.querySelector<HTMLElement>("#rv-detail")!;
    const splitRoot = this.container.querySelector<HTMLElement>("#rv-split")!;
    this.bulkBarRoot = this.container.querySelector<HTMLElement>("#rv-bulk-bar");

    const singleDetailRoot = this.container.querySelector<HTMLElement>("#rv-single-detail")!;
    this.mergedDetail = this.container.querySelector<HTMLElement>("#rv-merged-detail") as MergedConversationDetail;
    
    this.list = new RecordingsList(listRoot, this.state, (id) => this.onSelect(id), (ids) => {
      this.onSelectionChange(ids);
    });
    this.detail = new RecordingDetail(singleDetailRoot, () => {
      void this.refresh();
    });
    this.mergedDetail.onRefresh = () => {
      void this.refresh();
    };
    new Splitter(splitRoot, this.splitPercent, (pct) => {
      this.splitPercent = pct;
      this.applyLayout();
    });

    this.applyLayout();
    void this.refresh();
    void this.subscribeToEvents();
    this.keydownHandler = this.handleKeydown.bind(this);
    document.addEventListener("keydown", this.keydownHandler);
  }

  async refresh() {
    await this.list.refresh();
    const s = this.state.get();
    const selectedId = s.selectedId;
    if (selectedId && !s.recordings.some(r => r.id === selectedId || r.session_id === selectedId.replace("session:", ""))) {
      this.state.set({ ...s, selectedId: null });
      this.detail.clear();
      this.mergedDetail.sessionId = "";
    } else if (selectedId && !this.detail.hasDirtyEdits()) {
      if (selectedId.startsWith("session:")) {
        this.mergedDetail.sessionId = selectedId.substring(8);
      } else {
        void this.detail.show(selectedId);
      }
    }
  }

  toggleDetail() {
    this.detailVisible = !this.detailVisible;
    this.applyLayout();
  }

  private disposed = false;

  dispose() {
    this.disposed = true;
    if (this.unsub) {
      this.unsub();
      this.unsub = null;
    }
    document.removeEventListener("keydown", this.keydownHandler);
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
    const singleContainer = this.container.querySelector<HTMLElement>("#rv-single-detail")!;
    if (id.startsWith("session:")) {
      singleContainer.style.display = "none";
      this.mergedDetail.style.display = "block";
      this.detail.clear();
      this.mergedDetail.sessionId = id.substring(8);
    } else {
      this.mergedDetail.style.display = "none";
      singleContainer.style.display = "block";
      this.mergedDetail.sessionId = "";
      void this.detail.show(id);
    }
  }

  private onSelectionChange(ids: Set<string>) {
    this.multiSelected = ids;
    this.renderBulkBar();
  }

  private renderBulkBar() {
    const root = this.bulkBarRoot;
    if (!root) return;

    if (this.multiSelected.size === 0) {
      root.innerHTML = "";
      root.style.display = "none";
      return;
    }

    // Re-mount the BulkActionBar into the root element.
    new BulkActionBar(root, this.multiSelected, this.state.get().recordings, {
      onRefresh: () => { void this.refresh(); },
      onClear: () => {
        this.list.clearSelection();
        // clearSelection() will fire onSelectionChange(empty set) which hides the bar.
      },
    });
  }

  private async subscribeToEvents() {
    const unsub = await subscribe((event: DaemonEvent) => {
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
    // If the view was disposed while subscribe() was awaiting, unsubscribe
    // immediately so the daemon-event listener doesn't leak.
    if (this.disposed) {
      unsub();
      return;
    }
    this.unsub = unsub;
  }

  private async handleKeydown(e: KeyboardEvent) {
    // Ignore keydown if we are inside an input/textarea
    const target = e.target as HTMLElement;
    if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;

    if (e.ctrlKey && e.key === "\\") {
      e.preventDefault();
      this.toggleDetail();
    } else if (e.key === "Delete") {
      // If we have a multi-selection, bulk-delete via the bar; otherwise single-delete.
      if (this.multiSelected.size > 1) {
        // Delegate to BulkActionBar by programmatically clicking its delete button.
        const btn = this.bulkBarRoot?.querySelector<HTMLButtonElement>("#bulk-delete");
        btn?.click();
        return;
      }
      const id = this.state.get().selectedId;
      if (id) {
        e.preventDefault();
        const { confirmDelete } = await import("../ConfirmDelete");
        if (await confirmDelete()) {
          try {
            const { deleteRecording } = await import("../../services/ipc");
            await deleteRecording(id, false);
            this.refresh();
          } catch (err) {
            console.error("Failed to delete recording:", err);
          }
        }
      }
    }
  }
}
