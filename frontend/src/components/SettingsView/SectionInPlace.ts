import { invoke } from "@tauri-apps/api/core";
import { curatedSttModels } from "../../services/sttProviders";
import { curatedTranscriptionModels } from "../../data/curatedModels";
import { mountConnectionField } from "./connectionField";
import { mountModelField } from "./modelField";
import { bindFieldEvents, renderField } from "./form";
import { effectiveLocalWhisperHint, type WhisperPortStatus } from "./SectionWhisper";

const escHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** How `[in_place].stt` is being edited: absent = Automatic (the daemon falls
 *  back preview → main `[whisper]`), present = a pinned custom provider. */
type SttMode = "auto" | "custom";
/** Which already-running whisper server a custom LOCAL config points at. */
type LocalServer = "main" | "preview";

/**
 * Dictation (transcription-in-place) settings — the fast lane.
 *
 * By default an in-place dictation skips the queue and the full pipeline:
 * transcribe with a fast provider → instant rule-based polish → type at the
 * cursor, with the library save happening afterwards in the background. This
 * section tunes that behavior, including the dictation STT picker:
 * `in_place.stt` is the same optional-table shape as the Live Preview's
 * `preview_whisper`, so the Automatic↔Custom toggle here mirrors that
 * section's create/clear semantics exactly. One dictation-specific wrinkle:
 * the daemon never supervises a third whisper-server, so a custom LOCAL
 * config can only point at a server that's already running (the main one or
 * the preview's) — `in_place_provider_config()` mints a provider straight
 * from this table and local resolves via `server_base_url()`.
 */
export class SectionInPlace {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private config: any;
  private container: HTMLElement;
  /** Provider the shared model field was last mounted for (+ its host) — the
   *  mount-key guard, so repeat detail renders only reset the field when the
   *  provider actually changed (curated suggestions are per-provider). */
  private sttModelMountKey = "";
  private sttModelHost: HTMLElement | null = null;
  /** Live bundled-server ports from the daemon, fetched once on mount. Lets
   *  the local-server hint name the EFFECTIVE port after a port fallback; left
   *  null until the probe resolves (or when the daemon is down). */
  private portStatus: WhisperPortStatus | null = null;

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    this.config = config;
    this.container = container;
    this.render();
    void this.refreshPortStatus();
  }

  /** Probe the daemon for the bundled servers' effective ports, then re-render
   *  so the local-server hint reflects any fallback. Best-effort: a down daemon
   *  leaves the configured port showing. Calls `daemon_status` directly — the
   *  typed services wrapper drops the port fields, and this is pure display. */
  private async refreshPortStatus() {
    try {
      this.portStatus = await invoke<WhisperPortStatus>("daemon_status");
    } catch {
      this.portStatus = null;
      return;
    }
    // Only the local-server hint consumes the ports; skip a re-render otherwise.
    if (this.config.in_place?.stt && (this.config.in_place.stt.provider ?? "local") === "local") {
      this.render();
    }
  }

  private sttMode(): SttMode {
    return this.config.in_place?.stt ? "custom" : "auto";
  }

  private mainPort(): number {
    return (this.config.whisper?.bundled_server_port ?? 5809) as number;
  }

  /** Port of the live preview's dedicated server when it has one; else the
   *  conventional main+1 (what SectionPreview assigns its local config). */
  private previewPort(): number {
    const pv = this.config.preview_whisper;
    const port = pv?.provider === "local" ? pv?.bundled_server_port : undefined;
    return (port ?? this.mainPort() + 1) as number;
  }

  /** Which server the saved local config points at, for the select. Anything
   *  that isn't the preview server's port reads as "main" — both options
   *  below rewrite the table, so the two ports are all this UI ever writes. */
  private sttServer(): LocalServer {
    const stt = this.config.in_place?.stt;
    if (!stt || stt.mode === "external") return "main";
    return stt.bundled_server_port === this.previewPort() &&
      this.previewPort() !== this.mainPort()
      ? "preview"
      : "main";
  }

  /** The endpoint the daemon's `server_base_url()` resolves this config to —
   *  shown in the hint so "already running" is checkable at a glance. */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private sttLocalUrl(stt: any): string {
    return stt.mode === "external"
      ? String(stt.external_url ?? "").replace(/\/+$/, "")
      : `http://127.0.0.1:${stt.bundled_server_port ?? this.mainPort()}`;
  }

  /** Custom + local: a copy of the relevant server's `[whisper]`-shaped table
   *  (spread keeps every required field present, like SectionPreview does).
   *  "main" copies the main config verbatim — pointing wherever it points,
   *  bundled port or external URL alike. "preview" pins the preview server's
   *  port/model. No third server is spawned for dictation either way. */
  private setSttLocal(server: LocalServer) {
    const base = { ...(this.config.whisper ?? {}) };
    if (server === "preview") {
      const pv = this.config.preview_whisper ?? {};
      this.config.in_place.stt = {
        ...base,
        provider: "local",
        mode: "bundled_model",
        model_path: pv.model_path ?? "",
        bundled_server_port: this.previewPort(),
        api_key: "",
      };
    } else {
      this.config.in_place.stt = { ...base, provider: "local", api_key: "" };
    }
  }

  /** Custom + cloud, mirroring SectionPreview's setApi: spread the main
   *  config for the required fields, keep any key/model/url already typed so
   *  switching providers doesn't wipe them. (A saved key arrives masked from
   *  the daemon and round-trips as the sentinel unless retyped — same
   *  convention as every other api_key field.) */
  private setSttApi(provider: string) {
    const existing = this.config.in_place.stt ?? {};
    this.config.in_place.stt = {
      ...(this.config.whisper ?? {}),
      provider,
      mode: "external",
      model_path: "",
      api_key: existing.api_key ?? "",
      model: existing.model ?? "",
      api_url: existing.api_url ?? "",
    };
  }

  /** Automatic: drop the table entirely — `None` is what makes the daemon
   *  fall back preview → main (exactly how SectionPreview clears its own
   *  optional `preview_whisper` table). */
  private setSttAuto() {
    delete this.config.in_place.stt;
  }

  private render() {
    if (!this.config.in_place) {
      this.config.in_place = {
        cleanup: "fast",
        full_pipeline: false,
        type_first: false,
        save_to_library: true,
        type_mode: "type",
      };
    }
    const ip = this.config.in_place;
    const sttMode = this.sttMode();

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Dictation (in-place)</h3>
        <p style="font-size: 12px; color: var(--fg-muted); margin: 0 0 12px; line-height: 1.5;">
          The in-place hotkey types what you say straight into the focused window.
          Dictations take a <b>fast lane</b>: they skip the processing queue and the
          full pipeline, so the text lands in well under a second — even while a
          meeting is transcribing. The <b>Dictation model</b> below picks the STT
          provider; Automatic keeps it on the fastest one you've already set up.
        </p>

        <div class="settings-field">
          <label>Dictation model</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ip-stt-mode">
              <option value="auto" ${sttMode === "auto" ? "selected" : ""}>Automatic (preview's fast model, else the main one)</option>
              <option value="custom" ${sttMode === "custom" ? "selected" : ""}>Custom</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              <b>Automatic</b> needs no setup: dictation borrows the Live Preview's fast model
              while the preview is enabled (that server is already running), else the main
              transcription provider. <b>Custom</b> pins dictation to its own provider and model.
            </span>
          </div>
        </div>
        <div id="ip-stt-detail"></div>

        <div class="settings-field">
          <label>Text polish</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ip-cleanup">
              <option value="fast" ${(ip.cleanup ?? "fast") === "fast" ? "selected" : ""}>Fast — instant, rule-based (recommended)</option>
              <option value="off" ${ip.cleanup === "off" ? "selected" : ""}>Off — raw transcription</option>
              <option value="llm" ${ip.cleanup === "llm" ? "selected" : ""}>AI cleanup — slower, full LLM pass</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              <b>Fast</b> strips filler words ("um", "uh") and whisper's non-speech tags, fixes
              stutter-doubled words, capitalization, and end punctuation — with zero added latency.
              <b>AI cleanup</b> runs the Post-Processing provider before typing, adding its full
              round-trip time to every dictation.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Insert text by</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ip-type-mode">
              <option value="type" ${(ip.type_mode ?? "type") === "type" ? "selected" : ""}>Typing — simulated keystrokes</option>
              <option value="paste" ${ip.type_mode === "paste" ? "selected" : ""}>Pasting — clipboard + Ctrl+V</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Typing works everywhere but takes a moment for long text. Pasting is near-instant —
              your previous clipboard is put back afterwards — but a few apps block paste.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Keep dictations in the library</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "in_place.save_to_library", label: "", kind: "checkbox" },
              ip.save_to_library ?? true,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              On: after the text is typed, the recording saves like any other (searchable, with
              audio). Off: dictations are ephemeral — audio and transcript are discarded once typed.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Run the full pipeline</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "in_place.full_pipeline", label: "", kind: "checkbox" },
              ip.full_pipeline ?? false,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Route dictations through the normal queue and every configured step (cleanup,
              summary, auto-tags, hooks) — the pre-fast-lane behavior. Slow;
              only useful when dictations must trigger the same automation as recordings.
              <b>When to type</b> below picks whether the text waits for those steps.
            </span>
          </div>
        </div>
        ${
          ip.full_pipeline
            ? `
        <div class="settings-field">
          <label>When to type</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ip-type-first">
              <option value="immediate" ${ip.type_first ? "selected" : ""}>Type the text immediately — the pipeline keeps running in the background</option>
              <option value="after" ${!ip.type_first ? "selected" : ""}>Type only after every step finishes — the typed text includes cleanup</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              <b>Immediately</b> types the fast transcription the moment you stop speaking,
              while cleanup, summary, auto-tags, and hooks keep running in the background for
              the library copy — the typed text does <b>not</b> include the AI cleanup.
              <b>After every step</b> is the classic behavior: nothing lands at the cursor
              until the whole pipeline has finished, so the typed text includes cleanup.
            </span>
          </div>
        </div>`
            : ""
        }
      </div>
    `;

    bindFieldEvents(this.container, this.config);
    this.container
      .querySelector<HTMLSelectElement>("#ip-cleanup")
      ?.addEventListener("change", (e) => {
        this.config.in_place.cleanup = (e.target as HTMLSelectElement).value;
      });
    this.container
      .querySelector<HTMLSelectElement>("#ip-type-first")
      ?.addEventListener("change", (e) => {
        this.config.in_place.type_first = (e.target as HTMLSelectElement).value === "immediate";
      });
    // The "When to type" field only exists while the full-pipeline toggle is
    // on, so rebuild the section when it flips. bindFieldEvents (above) has
    // already written the new value by the time this fires — listeners run in
    // registration order — so the re-render sees the updated config.
    this.container
      .querySelector<HTMLInputElement>('input[data-key="in_place.full_pipeline"]')
      ?.addEventListener("change", () => this.render());
    this.container
      .querySelector<HTMLSelectElement>("#ip-type-mode")
      ?.addEventListener("change", (e) => {
        this.config.in_place.type_mode = (e.target as HTMLSelectElement).value;
      });
    this.container
      .querySelector<HTMLSelectElement>("#ip-stt-mode")
      ?.addEventListener("change", (e) => {
        const v = (e.target as HTMLSelectElement).value as SttMode;
        // Custom starts on the safest local choice — the main server, the one
        // the daemon always supervises; Automatic deletes the table.
        if (v === "auto") this.setSttAuto();
        else this.setSttLocal("main");
        this.render();
      });

    this.renderSttDetail();
  }

  private renderSttDetail() {
    const host = this.container.querySelector<HTMLElement>("#ip-stt-detail");
    if (!host) return;
    const stt = this.config.in_place?.stt;
    if (!stt) {
      host.innerHTML = "";
      return;
    }

    const isLocal = (stt.provider ?? "local") === "local";

    if (isLocal) {
      const server = this.sttServer();
      // Mirror of the daemon's preview_needs_own_server(): the second server
      // only exists while the preview is enabled AND set to a local model.
      const previewServerRuns =
        !!this.config.recording?.streaming_preview &&
        this.config.preview_whisper?.provider === "local";
      // Show the port the server ACTUALLY bound when the daemon reports a
      // fallback; the configured port stays the editable value. The note is
      // empty for external configs or when no fallback is known.
      const hint = effectiveLocalWhisperHint(this.sttLocalUrl(stt), this.portStatus);
      host.innerHTML = `
        <div class="settings-field conn-field">
          <label>Provider</label>
          <div id="ip-stt-conn"></div>
        </div>
        <div class="settings-field">
          <label>Local server</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ip-stt-server">
              <option value="main" ${server === "main" ? "selected" : ""}>Main transcription server</option>
              <option value="preview" ${server === "preview" ? "selected" : ""}>Live Preview's fast-model server</option>
            </select>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Dictation reuses a whisper server that's <b>already running</b> — the daemon never
              starts a third one just for it. <b>Main</b> is the regular transcription server;
              <b>Live Preview's</b> is the second, fast-model one, only alive while the preview
              is enabled with a dedicated local model. Requests go to ${escHtml(hint.url)}${hint.note ? ` ${escHtml(hint.note)}` : ""}.
            </span>
            ${
              server === "preview" && !previewServerRuns
                ? `<span style="font-size: 11px; color: var(--err); display: block;">The Live Preview isn't set to run its own local server right now — enable it with a dedicated local model (Transcription → Live Preview), or dictations will fail.</span>`
                : ""
            }
          </div>
        </div>`;
    } else {
      host.innerHTML = `
        <div class="settings-field conn-field">
          <label>Provider</label>
          <div id="ip-stt-conn"></div>
        </div>
        <div class="settings-field">
          <label>Model <span style="color:var(--fg-faded); font-weight:normal;">(optional)</span></label>
          <div id="ip-stt-model-host"></div>
        </div>`;
    }

    // The provider/key/endpoint rows are the shared connection block. Its
    // writes go through the same setters the old hand-rolled inputs used:
    // switching to local pins the safest choice (the main server — the one
    // the daemon always supervises), cloud providers go through setSttApi so
    // any key/model/url already typed survives the switch. Key/URL edits
    // write straight through with no re-render, so typing never resets the
    // mounted model field below.
    const connHost = host.querySelector<HTMLElement>("#ip-stt-conn");
    if (connHost) {
      mountConnectionField(connHost, {
        catalog: "stt",
        getKind: () => this.config.in_place?.stt?.provider ?? "local",
        setKind: (k: string) => {
          if (k === "local") this.setSttLocal("main");
          else this.setSttApi(k);
        },
        getApiUrl: () => this.config.in_place?.stt?.api_url ?? "",
        setApiUrl: (u: string) => {
          if (this.config.in_place?.stt) this.config.in_place.stt.api_url = u;
        },
        getApiKey: () => this.config.in_place?.stt?.api_key ?? "",
        setApiKey: (key: string) => {
          if (this.config.in_place?.stt) this.config.in_place.stt.api_key = key;
        },
        // Local↔cloud flips the rows below the block, so rebuild the section
        // (exactly what the old provider select's change handler did).
        onProviderChanged: () => this.render(),
        // Local resolves to the already-running server the config points at
        // (the daemon's server_base_url()); custom probes its endpoint.
        resolveTestUrl: () => {
          const cur = this.config.in_place?.stt;
          if (!cur) return "";
          return (cur.provider ?? "local") === "local"
            ? this.sttLocalUrl(cur)
            : String(cur.api_url ?? "").trim();
        },
      });
    }

    host.querySelector<HTMLSelectElement>("#ip-stt-server")?.addEventListener("change", (e) => {
      this.setSttLocal((e.target as HTMLSelectElement).value as LocalServer);
      this.render();
    });

    const modelHost = host.querySelector<HTMLElement>("#ip-stt-model-host");
    if (modelHost) this.mountSttModel(modelHost);
  }

  /** Mount the shared model field for the current cloud provider. The mount
   *  key skips the re-mount when neither the host nor the provider changed —
   *  only a provider switch should reset the field, since the curated
   *  suggestions (same sources the Live Preview uses) are per-provider. */
  private mountSttModel(modelHost: HTMLElement) {
    const key = String(this.config.in_place?.stt?.provider ?? "");
    if (modelHost === this.sttModelHost && key === this.sttModelMountKey) return;
    this.sttModelHost = modelHost;
    this.sttModelMountKey = key;
    mountModelField(modelHost, {
      mode: "curated",
      getProvider: () => this.config.in_place?.stt?.provider ?? "",
      getApiUrl: () => this.config.in_place?.stt?.api_url ?? "",
      getApiKey: () => this.config.in_place?.stt?.api_key ?? "",
      getModel: () => this.config.in_place?.stt?.model ?? "",
      setModel: (m) => {
        if (this.config.in_place?.stt) this.config.in_place.stt.model = m;
      },
      curated: () => curatedSttModels(this.config.in_place?.stt?.provider ?? ""),
      curatedRich: () => curatedTranscriptionModels(this.config.in_place?.stt?.provider ?? ""),
    });
  }
}
