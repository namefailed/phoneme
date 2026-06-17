import { LitElement, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import {
  listRecordings,
  retranscribeRecording,
  clearFailed,
  getQueueCounts,
  type Recording,
} from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { formatDuration } from "../../utils/format";
import { showToast } from "../../utils/toast";
import { errText } from "../../utils/error";
import "../modal.css";

/** Which pipeline step a live failure event came from. */
export type FailureStage = "transcribe" | "hook";

/** Error text captured from a failure event, with when we saw it. */
type FailureDetail = { stage: FailureStage; error: string; at: number };

/**
 * Session-level cache of failure details, keyed by recording id. The daemon now
 * persists the reason on the row (`error_kind`/`error_message`), so the stored
 * value is authoritative and survives a restart — `failureMessage` prefers it.
 * This cache is the fallback for the window between a live
 * `transcription_failed` / `hook_failed` event and the next catalog refresh: the
 * always-mounted queue panel feeds it from its event subscription so a failure
 * that happens while the app is open shows its actual message immediately.
 */
const sessionFailures = new Map<string, FailureDetail>();

/** Record the error text from a live failure event (called by the queue
 *  panel's subscription, which is mounted for the app's whole lifetime). */
export function recordFailureDetail(id: string, stage: FailureStage, error: string): void {
  sessionFailures.set(id, { stage, error, at: Date.now() });
}

/** Statuses that mean "this recording's pipeline failed permanently". The
 *  catalog filter is an exact match, so the panel queries each separately.
 *  Includes the optional-step failures (cleanup/summary/title/tag): the
 *  transcript is intact, but the enrichment failed and is worth triaging. */
const FAILED_STATUSES = [
  "transcribe_failed",
  "hook_failed",
  "cleanup_failed",
  "summarize_failed",
  "title_failed",
  "tag_failed",
] as const;

/** Cap per status query — keeps the panel snappy; nobody triages more. */
const LIST_LIMIT = 100;

/** Display title for a row: the recording's title, else its start time. */
function rowTitle(r: Recording): string {
  return r.title?.trim() || new Date(r.started_at).toLocaleString();
}

/** Which step failed, as a short label. The failed status is the reliable
 *  source (every failure has one); the stored `error_kind` is a fallback for
 *  older rows or unexpected statuses. */
function failureStage(r: Recording): string {
  switch (r.status) {
    case "hook_failed":       return "Hook";
    case "cleanup_failed":    return "Cleanup";
    case "summarize_failed":  return "Summary";
    case "title_failed":      return "Title";
    case "tag_failed":        return "Tagging";
    case "transcribe_failed": return "Transcription";
  }
  const kind = r.error_kind?.trim();
  if (kind) {
    if (kind === "whisper_error") return "Transcription";
    if (kind.startsWith("hook")) return "Hook";
    return kind.replace(/_/g, " ");
  }
  return "Transcription";
}

/** The best error text we have for a row, and whether it's a real message
 *  (vs. the "nothing captured" fallback, styled quieter). */
function failureMessage(r: Recording): { text: string; known: boolean } {
  const stored = r.error_message?.trim();
  if (stored) return { text: stored, known: true };
  const live = sessionFailures.get(r.id);
  if (live) return { text: live.error, known: true };
  return {
    text:
      "No error detail captured — this failure predates this app session. " +
      "The full story is in the daemon log: %LOCALAPPDATA%\\phoneme\\logs\\daemon.log",
    known: false,
  };
}

/** When the failure happened, best-effort: the live event time when we saw
 *  one, the hook run time for hook failures, else the recording time (an
 *  honest "recorded …" — the catalog doesn't store a failed-at). */
function failureWhen(r: Recording): string {
  const live = sessionFailures.get(r.id);
  if (live) return `failed ${new Date(live.at).toLocaleString()}`;
  if (r.status === "hook_failed" && r.hook_ran_at) {
    return `failed ${new Date(r.hook_ran_at).toLocaleString()}`;
  }
  return `recorded ${new Date(r.started_at).toLocaleString()}`;
}

/**
 * Failure-details panel, opened from the queue header's "⚠ N failed" badge.
 * One row per failed recording (catalog rows with a failed status): which step
 * failed, the error text (selectable), when — with per-row Retry (re-runs the
 * whole pipeline) and Open (jump to the recording). Footer: Retry all
 * (sequential, with progress) and the existing Clear-failed quarantine action.
 *
 * Uses the house modal idiom (`.modal-overlay`): both global keyboard layers
 * stand down while one is open, so Escape closes the panel — never the open
 * recording behind it.
 */
@customElement("ph-failed-panel")
export class FailedPanelElement extends LitElement {
  protected createRenderRoot() {
    return this; // light DOM for global CSS / theme vars
  }

  @state() private rows: Recording[] = [];
  @state() private loading = true;
  @state() private error: string | null = null;
  /** Ids with a single Retry in flight (row button shows "Retrying…"). */
  @state() private retrying = new Set<string>();
  /** Retry-all progress, or null when idle. Disables all actions while set. */
  @state() private retryAll: { done: number; total: number } | null = null;
  /** True while the clear-failed request runs. */
  @state() private clearing = false;
  /** Inbox `failed/` quarantine depth — what Clear failed actually empties.
   *  Distinct from `rows` (catalog statuses): clearing the quarantine does
   *  NOT un-fail the recordings, and a retry doesn't shrink the quarantine. */
  @state() private inboxFailed = 0;

  private unsub: (() => void) | null = null;

  private keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape") this.close();
  };

  async connectedCallback() {
    super.connectedCallback();
    document.addEventListener("keydown", this.keyHandler);
    void this.load();
    const unsub = await subscribe((event: DaemonEvent) => {
      // New failures land while the panel is open: capture the live error
      // text first, then refresh so the row appears with it.
      if (event.event === "transcription_failed") {
        recordFailureDetail(event.id, "transcribe", event.error);
        void this.load();
        return;
      }
      if (event.event === "hook_failed") {
        recordFailureDetail(event.id, "hook", event.error);
        void this.load();
        return;
      }
      // A retry (from here or anywhere) succeeding, or a delete, drops rows.
      if (
        event.event === "transcription_done" ||
        event.event === "transcript_updated" ||
        event.event === "hook_done" ||
        event.event === "recording_deleted"
      ) {
        void this.load();
      }
      // Keep the Clear-failed count honest (fires on clear + on new failures).
      if (event.event === "queue_depth_changed") {
        this.inboxFailed = event.failed;
      }
    });
    // If the element disconnected while the subscription was awaiting (a fast
    // open-then-close), disconnectedCallback already ran with this.unsub null —
    // tear the late listener down now instead of leaking it.
    if (!this.isConnected) unsub();
    else this.unsub = unsub;
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.keyHandler);
    if (this.unsub) this.unsub();
  }

  firstUpdated() {
    this.querySelector<HTMLButtonElement>(".failed-close")?.focus();
  }

  private close() {
    this.dispatchEvent(new CustomEvent("resolved"));
  }

  private async load() {
    // Retry-all owns the list while it runs (rows leave one by one as each
    // retry queues); it reloads itself when the sweep finishes.
    if (this.retryAll !== null) return;
    // No spinner churn on refreshes — only the very first load shows one.
    this.error = null;
    try {
      // The catalog's status filter is an exact match, so "failed" is two
      // queries (transcribe_failed + hook_failed), merged newest-first.
      const lists = await Promise.all(
        FAILED_STATUSES.map((status) => listRecordings({ status, limit: LIST_LIMIT })),
      );
      const merged = lists
        .flat()
        .sort((a, b) => (a.started_at < b.started_at ? 1 : a.started_at > b.started_at ? -1 : 0));
      // Don't resurrect a row whose retry is mid-flight (the daemon flips the
      // status to transcribing before the IPC returns, but an event-driven
      // refresh can race the click).
      this.rows = merged.filter((r) => !this.retrying.has(r.id));
    } catch (e) {
      this.error = errText(e);
      this.rows = [];
    } finally {
      this.loading = false;
    }
    try {
      this.inboxFailed = (await getQueueCounts()).failed;
    } catch {
      /* leave last-known count */
    }
  }

  /** Re-run the whole pipeline for one recording (the v1 retry) and drop the
   *  row optimistically — the daemon marks it transcribing before returning. */
  private async retry(id: string) {
    if (this.retrying.has(id) || this.retryAll !== null) return;
    this.retrying = new Set(this.retrying).add(id);
    try {
      await retranscribeRecording(id);
      sessionFailures.delete(id);
      this.rows = this.rows.filter((r) => r.id !== id);
      showToast("Queued for re-transcription", "info");
    } catch (e) {
      showToast(`Couldn't retry: ${errText(e)}`, "error");
    } finally {
      const next = new Set(this.retrying);
      next.delete(id);
      this.retrying = next;
    }
  }

  /** Retry every listed recording, one at a time (the queue is serial anyway;
   *  sequential keeps the daemon honest and the progress count meaningful). */
  private async retryAllRows() {
    if (this.retryAll !== null || this.rows.length === 0) return;
    const ids = this.rows.map((r) => r.id);
    this.retryAll = { done: 0, total: ids.length };
    let failures = 0;
    for (const id of ids) {
      try {
        await retranscribeRecording(id);
        sessionFailures.delete(id);
        this.rows = this.rows.filter((r) => r.id !== id);
      } catch {
        failures++;
      }
      this.retryAll = { done: this.retryAll.done + 1, total: ids.length };
    }
    this.retryAll = null;
    if (failures > 0) {
      showToast(`Retried ${ids.length - failures} of ${ids.length} — ${failures} couldn't be queued`, "error");
    } else {
      showToast(`Queued ${ids.length} recording${ids.length === 1 ? "" : "s"} for re-transcription`, "info");
    }
    void this.load();
  }

  /** Clear the inbox `failed/` quarantine — the queue badge's count. The
   *  recordings keep their failed status and stay in this list. */
  private async clearQuarantine() {
    if (this.inboxFailed === 0 || this.clearing || this.retryAll !== null) return;
    const n = this.inboxFailed;
    if (
      !window.confirm(
        `Clear ${n} failed item${n === 1 ? "" : "s"} from the queue badge? ` +
          "This only resets the queue's failure marker — the recordings keep " +
          "their Failed status and stay visible here and in the library.",
      )
    ) {
      return;
    }
    this.clearing = true;
    try {
      const removed = await clearFailed();
      this.inboxFailed = 0;
      showToast(`Cleared ${removed} failed item${removed === 1 ? "" : "s"} from the badge`, "info");
    } catch (e) {
      showToast(`Couldn't clear failed: ${errText(e)}`, "error");
    } finally {
      this.clearing = false;
    }
  }

  /** Jump to this recording in the list + detail view (same event the queue
   *  rows use), and close the panel so the detail pane is visible. */
  private open(id: string) {
    window.dispatchEvent(new CustomEvent("phoneme:select-recording", { detail: { id } }));
    this.close();
  }

  private renderRow(r: Recording) {
    const msg = failureMessage(r);
    const busy = this.retrying.has(r.id) || this.retryAll !== null;
    return html`
      <div class="failed-row" tabindex="0" role="group" aria-label=${`Failed recording: ${rowTitle(r)}`}>
        <div class="failed-main">
          <div class="failed-row-head">
            <span class="failed-title">${rowTitle(r)}</span>
            <span class="failed-stage">${failureStage(r)}</span>
          </div>
          <div class="failed-msg ${msg.known ? "" : "unknown"}">${msg.text}</div>
          <div class="failed-when">${failureWhen(r)} · ${formatDuration(r.duration_ms)}</div>
        </div>
        <div class="failed-row-actions">
          <button
            class="modal-btn failed-retry"
            ?disabled=${busy}
            title="Re-run the whole pipeline for this recording"
            @click=${() => void this.retry(r.id)}
          >
            ${this.retrying.has(r.id) ? "Retrying…" : "Retry"}
          </button>
          <button
            class="modal-btn failed-open"
            title="Open this recording in the library"
            @click=${() => this.open(r.id)}
          >
            Open
          </button>
        </div>
      </div>
    `;
  }

  render() {
    const n = this.rows.length;
    const busy = this.retryAll !== null;
    return html`
      <div
        class="modal-overlay"
        @click=${(e: MouseEvent) => {
          if (e.target === e.currentTarget) this.close();
        }}
      >
        <div class="modal-dialog failed-dialog" role="dialog" aria-modal="true" aria-labelledby="failed-title">
          <div class="modal-header">
            <h3 class="modal-title" id="failed-title">⚠ Failed recordings</h3>
            ${n ? html`<span class="failed-count-chip">${n}</span>` : nothing}
          </div>

          <div class="failed-body">
            ${this.loading
              ? html`<div class="failed-empty">Loading…</div>`
              : this.error
                ? html`<div class="failed-empty err">${this.error}</div>`
                : n === 0
                  ? html`<div class="failed-empty">
                      Nothing has failed. Recordings that hit a permanent transcription or hook error show up
                      here with the reason.
                    </div>`
                  : this.rows.map((r) => this.renderRow(r))}
          </div>

          <div class="modal-actions failed-actions">
            <span class="failed-foot-note">
              ${busy
                ? `Retrying ${this.retryAll!.done}/${this.retryAll!.total}…`
                : "Retry re-runs the whole pipeline."}
            </span>
            <button
              class="modal-btn failed-retry-all"
              ?disabled=${busy || n === 0}
              title="Re-queue every listed recording, one at a time"
              @click=${() => void this.retryAllRows()}
            >
              ${busy ? `Retrying ${this.retryAll!.done}/${this.retryAll!.total}…` : `Retry all (${n})`}
            </button>
            <button
              class="modal-btn failed-clear"
              ?disabled=${this.inboxFailed === 0 || this.clearing || busy}
              title="Reset the queue's failed badge — the recordings keep their Failed status"
              @click=${() => void this.clearQuarantine()}
            >
              ${this.clearing ? "Clearing…" : `Clear failed${this.inboxFailed ? ` (${this.inboxFailed})` : ""}`}
            </button>
            <button class="modal-btn modal-btn-primary failed-close" @click=${() => this.close()}>Close</button>
          </div>
        </div>
      </div>
    `;
  }
}

/** Open the failure-details panel; resolves when it closes. */
export async function openFailedPanel(): Promise<void> {
  return new Promise((resolve) => {
    document.querySelector("ph-failed-panel")?.remove();
    const el = document.createElement("ph-failed-panel") as FailedPanelElement;
    el.addEventListener("resolved", () => {
      el.remove();
      resolve();
    });
    document.body.appendChild(el);
  });
}
