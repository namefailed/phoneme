import { escapeHtml as escHtml } from "../../utils/format";
import { invoke } from "@tauri-apps/api/core";
import { curatedSttModels } from "../../services/sttProviders";
import { curatedTranscriptionModels } from "../../data/curatedModels";
import { mountConnectionField } from "./connectionField";
import { mountModelField } from "./modelField";
import { bindFieldEvents, renderField } from "./form";
import { effectiveLocalWhisperHint, type WhisperPortStatus } from "./SectionWhisper";


/** Friendly label for a downloaded whisper model filename (mirrors the Live
 *  Preview section's local dropdown, so the dedicated dictation picker reads
 *  the same way). */
function prettyModel(path: string): string {
  const name = path.replace(/\\/g, "/").split("/").pop() ?? path;
  const map: Record<string, string> = {
    "ggml-tiny.en.bin": "Tiny (English)",
    "ggml-base.en.bin": "Base (English)",
    "ggml-small.en.bin": "Small (English)",
    "ggml-medium.en.bin": "Medium (English)",
    "ggml-large-v3.bin": "Large v3",
    "ggml-large-v3-turbo.bin": "Large v3 Turbo",
    "ggml-large-v3-turbo-q5_0.bin": "Large v3 Turbo (q5)",
  };
  return map[name] ?? name;
}

/** How `[in_place].stt` is being edited: absent = Automatic (the daemon falls
 *  back preview → main `[whisper]`), present = a pinned custom provider. */
type SttMode = "auto" | "custom";
/** Which whisper server a custom local config uses: an already-running one
 *  (`main` / `preview`), or a `dedicated` third server the daemon supervises
 *  just for dictation (the power-user opt-in, which uses extra RAM). */
type LocalServer = "main" | "preview" | "dedicated";

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
 * by default the daemon won't supervise a third whisper-server, so a custom
 * local config usually points at one that's already running (the main one or
 * the preview's). `in_place_provider_config()` mints a provider straight from
 * this table, and local resolves via `server_base_url()`.
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
  /** Downloaded whisper models, for the dedicated-server model dropdown (same
   *  list the Live Preview's local picker uses). Fetched lazily the first time
   *  the dedicated server is active; populated into the select on resolve. */
  private downloaded: string[] = [];
  /** True once the downloaded-models list has been fetched, so we only probe
   *  once (the list doesn't change while the section is open — new downloads
   *  happen in the Whisper section). */
  private downloadedLoaded = false;

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

  /** True when the main transcription model is a heavy local model (medium /
   *  large) — pointing dictation at it (Custom → Main) makes dictations slow.
   *  A cloud main model has no local cost to borrow, so it's never "heavy". */
  private mainModelIsHeavy(): boolean {
    const w = this.config.whisper;
    if (!w) return false;
    if (w.provider && w.provider !== "local") return false;
    return /medium|large/i.test(String(w.model_path ?? ""));
  }

  /** Port of the live preview's dedicated server when it has one; else the
   *  conventional main+1 (what SectionPreview assigns its local config). */
  private previewPort(): number {
    const pv = this.config.preview_whisper;
    const port = pv?.provider === "local" ? pv?.bundled_server_port : undefined;
    return (port ?? this.mainPort() + 1) as number;
  }

  /** Port for the dedicated dictation server — distinct from main and the
   *  preview's (main+2 by convention, the documented 5811 next to 5809/5810).
   *  resolve()/apply() route to it only when it differs from main/preview, so
   *  the third server is actually dialed instead of reusing an existing one. */
  private dedicatedPort(): number {
    return (this.mainPort() + 2) as number;
  }

  /** Which server the saved local config points at, for the select. The opt-in
   *  flag identifies a dedicated third server; otherwise the preview port reads
   *  as "preview" and everything else as "main" (the reuse cases). */
  private sttServer(): LocalServer {
    const stt = this.config.in_place?.stt;
    if (!stt || stt.mode === "external") return "main";
    if (stt.use_own_bundled_server) return "dedicated";
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
   *
   *  - "main"      — copies the main config verbatim, reusing the always-on
   *                  server. Clears the opt-in flag.
   *  - "preview"   — pins the preview server's port/model, reusing it. Clears
   *                  the flag.
   *  - "dedicated" — the power-user opt-in: its own port (main+2), its own
   *                  model, and `use_own_bundled_server = true`, so the daemon
   *                  supervises a third server and dictation actually dials it.
   *                  Uses extra RAM. */
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
        use_own_bundled_server: false,
        api_key: "",
      };
    } else if (server === "dedicated") {
      // Keep a model already chosen for the dictation server (so flipping the
      // toggle back and forth doesn't wipe the pick); else start from the
      // preview's fast model if there is one, falling back to blank.
      const existing = this.config.in_place?.stt;
      const seedModel =
        existing?.use_own_bundled_server && existing?.model_path
          ? existing.model_path
          : (this.config.preview_whisper?.model_path ?? "");
      this.config.in_place.stt = {
        ...base,
        provider: "local",
        mode: "bundled_model",
        model_path: seedModel,
        bundled_server_port: this.dedicatedPort(),
        use_own_bundled_server: true,
        api_key: "",
      };
    } else {
      this.config.in_place.stt = {
        ...base,
        provider: "local",
        use_own_bundled_server: false,
        api_key: "",
      };
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
      // A cloud provider never has a daemon-supervised server.
      use_own_bundled_server: false,
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
        stream_type: false,
      };
    }
    const ip = this.config.in_place;
    // Seed the phase-2 tables so the editors below always have something to bind
    // to (a config saved before these existed simply lacks the keys). Empty map +
    // context off = today's behavior unchanged.
    if (!ip.app_overrides || typeof ip.app_overrides !== "object") ip.app_overrides = {};
    if (!Array.isArray(ip.app_context_denylist)) ip.app_context_denylist = [];
    if (typeof ip.app_context !== "boolean") ip.app_context = false;
    const sttMode = this.sttMode();

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Dictation engine</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin: 0 0 12px; line-height: 1.5;">
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
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              <b>Automatic</b> needs no setup: dictation borrows the Live Preview's fast model
              while the preview is enabled (that server is already running), else the main
              transcription provider. <b>Custom</b> pins dictation to its own provider and model —
              point it at your main model for higher accuracy (slower, since that's the large one),
              or at a cloud API (e.g. Groq) for dictation that's fast <i>and</i> accurate.
              Phoneme runs at most <b>two</b> local whisper models at once — your main one and the
              Live Preview's fast one — so a local Custom choice reuses one of those servers rather
              than loading a third. (A separate cloud provider here doesn't count against that.)
            </span>
          </div>
        </div>
        <div id="ip-stt-detail"></div>
      </div>

      <div class="settings-section">
        <h3>Text delivery</h3>

        <div class="settings-field">
          <label>Text polish</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <select id="ip-cleanup">
              <option value="fast" ${(ip.cleanup ?? "fast") === "fast" ? "selected" : ""}>Fast — instant, rule-based (recommended)</option>
              <option value="off" ${ip.cleanup === "off" ? "selected" : ""}>Off — raw transcription</option>
              <option value="llm" ${ip.cleanup === "llm" ? "selected" : ""}>AI cleanup — slower, full LLM pass</option>
            </select>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
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
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              Typing works everywhere but takes a moment for long text. Pasting is near-instant —
              your previous clipboard is put back afterwards — but a few apps block paste.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Stream as you speak <span style="font-size:0.7143rem; font-weight:600; color:var(--accent); border:1px solid color-mix(in srgb, var(--accent) 35%, transparent); border-radius:6px; padding:0 5px; vertical-align:middle;">experimental</span></label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "in_place.stream_type", label: "", kind: "checkbox" },
              ip.stream_type ?? false,
            )}</div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              Off by default. With delivery set to <b>Typing</b>, dictated words appear live at your
              cursor as you speak, then a quiet patch corrects them to the accurate final transcript
              when you stop. It types the live preview's words, so it reads best with a fast preview
              model; ignored when delivery is <b>Pasting</b>.
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Per-app delivery</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 8px; width: 100%;">
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              Override how dictation lands for specific apps, by the app's executable name (e.g.
              <code>Code.exe</code> or just <code>code</code> — matched case-insensitively against the
              window focused when you stop speaking). <b>Type</b> / <b>Paste</b> as above, or <b>Off</b>
              to not auto-insert text for that app at all (the dictation still saves to the library).
              Apps not listed use the default <b>Insert text by</b> setting above.
            </span>
            <div id="ip-app-overrides" style="display: flex; flex-direction: column; gap: 6px; width: 100%;"></div>
            <div style="display: flex; gap: 6px; width: 100%; align-items: center;">
              <input id="ip-app-add-name" type="text" placeholder="App executable (e.g. Code.exe)"
                style="flex: 1 1 auto; min-width: 0;" />
              <select id="ip-app-add-mode">
                <option value="type">Type</option>
                <option value="paste">Paste</option>
                <option value="off">Off</option>
              </select>
              <button id="ip-app-add-btn" type="button">Add</button>
            </div>
          </div>
        </div>

        <div class="settings-field">
          <label>App-aware AI cleanup</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "in_place.app_context", label: "", kind: "checkbox" },
              ip.app_context ?? false,
            )}</div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              Off by default. When on, the title of the focused window is added to the
              <b>AI cleanup</b> prompt (only when Text polish is set to <b>AI cleanup</b>) so the LLM
              can adapt — for example, leaning code-ish in an editor. <b>Privacy:</b> the window title
              can be sensitive (a document name, an email subject), and turning this on means it is
              <b>sent to your configured cleanup provider</b> — prefer a local LLM if that matters.
              It is never logged or stored. While off, the title is never even read.
            </span>
          </div>
        </div>
        ${
          ip.app_context
            ? `
        <div class="settings-field">
          <label>Never read titles from</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 8px; width: 100%;">
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              Apps (by executable name) whose window titles are <b>never</b> read for context, even
              while App-aware cleanup is on — e.g. a password manager or a banking app.
            </span>
            <div id="ip-context-denylist" style="display: flex; flex-direction: column; gap: 6px; width: 100%;"></div>
            <div style="display: flex; gap: 6px; width: 100%; align-items: center;">
              <input id="ip-deny-add-name" type="text" placeholder="App executable (e.g. 1Password.exe)"
                style="flex: 1 1 auto; min-width: 0;" />
              <button id="ip-deny-add-btn" type="button">Add</button>
            </div>
          </div>
        </div>`
            : ""
        }
      </div>

      <div class="settings-section">
        <h3>Dictation pipeline</h3>

        <div class="settings-field">
          <label>Keep dictations in the library</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "in_place.save_to_library", label: "", kind: "checkbox" },
              ip.save_to_library ?? true,
            )}</div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
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
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
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
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
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
    // App-aware cleanup toggles whether the denylist editor shows; rebuild so it
    // appears/hides. bindFieldEvents already wrote the new boolean before this.
    this.container
      .querySelector<HTMLInputElement>('input[data-key="in_place.app_context"]')
      ?.addEventListener("change", () => this.render());

    this.renderAppOverrides();
    this.renderContextDenylist();
    this.renderSttDetail();
  }

  /** Render the per-app delivery rows (app name + mode + remove) and wire the
   *  Add control. Each row writes straight into `in_place.app_overrides`; the
   *  daemon keys it by the lowercased executable stem at typing time. */
  private renderAppOverrides() {
    const host = this.container.querySelector<HTMLElement>("#ip-app-overrides");
    if (!host) return;
    const overrides: Record<string, string> = this.config.in_place.app_overrides ?? {};
    const names = Object.keys(overrides).sort();
    host.innerHTML =
      names.length === 0
        ? `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No per-app overrides — every app uses the default above.</span>`
        : names
            .map(
              (name) => `
        <div class="ip-app-row" data-name="${escHtml(name)}"
          style="display: flex; gap: 6px; width: 100%; align-items: center;">
          <span style="flex: 1 1 auto; min-width: 0; font-family: var(--font-mono, monospace); overflow: hidden; text-overflow: ellipsis;">${escHtml(name)}</span>
          <select class="ip-app-mode" data-name="${escHtml(name)}">
            <option value="type" ${overrides[name] === "type" ? "selected" : ""}>Type</option>
            <option value="paste" ${overrides[name] === "paste" ? "selected" : ""}>Paste</option>
            <option value="off" ${overrides[name] === "off" ? "selected" : ""}>Off</option>
          </select>
          <button class="ip-app-remove" type="button" data-name="${escHtml(name)}" title="Remove">✕</button>
        </div>`,
            )
            .join("");

    host.querySelectorAll<HTMLSelectElement>(".ip-app-mode").forEach((sel) => {
      sel.addEventListener("change", () => {
        const name = sel.getAttribute("data-name");
        if (name) this.config.in_place.app_overrides[name] = sel.value;
      });
    });
    host.querySelectorAll<HTMLButtonElement>(".ip-app-remove").forEach((btn) => {
      btn.addEventListener("click", () => {
        const name = btn.getAttribute("data-name");
        if (name) delete this.config.in_place.app_overrides[name];
        this.renderAppOverrides();
      });
    });

    const nameInput = this.container.querySelector<HTMLInputElement>("#ip-app-add-name");
    const modeSel = this.container.querySelector<HTMLSelectElement>("#ip-app-add-mode");
    const addBtn = this.container.querySelector<HTMLButtonElement>("#ip-app-add-btn");
    const add = () => {
      // Store the stem lowercased — the daemon matches the focused process's
      // lowercased file stem, so "Code.exe" / "code" / "CODE" all normalize.
      const raw = (nameInput?.value ?? "").trim().replace(/\.exe$/i, "");
      if (!raw) return;
      this.config.in_place.app_overrides[raw.toLowerCase()] = modeSel?.value ?? "type";
      if (nameInput) nameInput.value = "";
      this.renderAppOverrides();
    };
    addBtn?.addEventListener("click", add);
    nameInput?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        add();
      }
    });
  }

  /** Render the app-context denylist rows + Add control. Only present while
   *  App-aware cleanup is on (the section is rebuilt when that flips). */
  private renderContextDenylist() {
    const host = this.container.querySelector<HTMLElement>("#ip-context-denylist");
    if (!host) return;
    const deny: string[] = this.config.in_place.app_context_denylist ?? [];
    host.innerHTML =
      deny.length === 0
        ? `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No apps excluded — titles may be read from any focused app.</span>`
        : deny
            .map(
              (name, i) => `
        <div style="display: flex; gap: 6px; width: 100%; align-items: center;">
          <span style="flex: 1 1 auto; min-width: 0; font-family: var(--font-mono, monospace); overflow: hidden; text-overflow: ellipsis;">${escHtml(name)}</span>
          <button class="ip-deny-remove" type="button" data-idx="${i}" title="Remove">✕</button>
        </div>`,
            )
            .join("");

    host.querySelectorAll<HTMLButtonElement>(".ip-deny-remove").forEach((btn) => {
      btn.addEventListener("click", () => {
        const idx = Number(btn.getAttribute("data-idx"));
        if (!Number.isNaN(idx)) this.config.in_place.app_context_denylist.splice(idx, 1);
        this.renderContextDenylist();
      });
    });

    const nameInput = this.container.querySelector<HTMLInputElement>("#ip-deny-add-name");
    const addBtn = this.container.querySelector<HTMLButtonElement>("#ip-deny-add-btn");
    const add = () => {
      const raw = (nameInput?.value ?? "").trim().replace(/\.exe$/i, "");
      if (!raw) return;
      const stem = raw.toLowerCase();
      if (!this.config.in_place.app_context_denylist.includes(stem)) {
        this.config.in_place.app_context_denylist.push(stem);
      }
      if (nameInput) nameInput.value = "";
      this.renderContextDenylist();
    };
    addBtn?.addEventListener("click", add);
    nameInput?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        add();
      }
    });
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
      // Show the port the server really bound when the daemon reports a
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
              <option value="main" ${server === "main" ? "selected" : ""}>Main transcription server (reuse)</option>
              <option value="preview" ${server === "preview" ? "selected" : ""}>Live Preview's fast-model server (reuse)</option>
              <option value="dedicated" ${server === "dedicated" ? "selected" : ""}>Dedicated dictation server (power user — extra RAM)</option>
            </select>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              <b>Reuse</b> a whisper server that's <b>already running</b> — the daemon starts no
              extra process. <b>Main</b> is the regular transcription server; <b>Live Preview's</b>
              is the second, fast-model one (only alive while the preview is on with a local model).
              <b>Dedicated</b> runs a <i>third</i> whisper-server just for dictation on its own
              port — the fastest, most reliable dictation, but it loads another model into RAM.
              Requests go to ${escHtml(hint.url)}${hint.note ? ` ${escHtml(hint.note)}` : ""}.
            </span>
            ${
              server === "preview" && !previewServerRuns
                ? `<span style="font-size: 0.7857rem; color: var(--err); display: block;">The Live Preview isn't set to run its own local server right now — enable it with a dedicated local model (Transcription → Live Preview), or dictations will fail.</span>`
                : ""
            }
            ${
              server === "main" && this.mainModelIsHeavy()
                ? `<div style="margin-top:8px; padding:8px 10px; border-left:3px solid var(--accent, #89b4fa); background:color-mix(in srgb, var(--accent, #89b4fa) 12%, transparent); border-radius:6px; font-size: 0.7857rem; color:var(--fg-default); line-height:1.5;">⚠️ Your main transcription model is large, so dictation through it will be slow. For fast dictation, switch to <b>Automatic</b> (above), the Live Preview's fast-model server, or a <b>Dedicated</b> server with a small model; for fast <i>and</i> accurate, point Custom at a cloud provider (e.g. Groq).</div>`
                : ""
            }
            ${
              server === "dedicated"
                ? `<div style="margin-top:8px; padding:8px 10px; border-left:3px solid var(--accent, #89b4fa); background:color-mix(in srgb, var(--accent, #89b4fa) 12%, transparent); border-radius:6px; font-size: 0.7857rem; color:var(--fg-default); line-height:1.5;">A third whisper-server runs alongside the main one (and the preview's, if on). Pick a <b>small, fast model</b> (tiny / base) below — running Turbo + Tiny + a heavy dictation model all at once needs plenty of free RAM. On a weak machine, prefer <b>Reuse</b> instead.</div>`
                : ""
            }
          </div>
        </div>
        ${
          server === "dedicated"
            ? `<div class="settings-field">
          <label>Dictation model</label>
          <div id="ip-stt-dedicated-model-host"></div>
        </div>`
            : ""
        }`;
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

    // Dedicated dictation server: a dropdown of downloaded GGML models for the
    // third server (the same local picker the Live Preview uses). Models are
    // downloaded in the Whisper section; a small model keeps dictation fast and
    // the RAM cost down.
    const dedicatedModelHost = host.querySelector<HTMLElement>("#ip-stt-dedicated-model-host");
    if (dedicatedModelHost && this.config.in_place?.stt) {
      this.renderDedicatedModelSelect(dedicatedModelHost);
    }

    const modelHost = host.querySelector<HTMLElement>("#ip-stt-model-host");
    if (modelHost) this.mountSttModel(modelHost);
  }

  /** Render the dedicated-server model picker as a dropdown of downloaded
   *  models (mirrors SectionPreview's `src === "local"` branch): an <option>
   *  per downloaded model with the current `model_path` pre-selected (matched
   *  on the normalized filename), writing the full path on change. Fetches the
   *  list lazily the first time and re-renders on resolve; shows an empty-state
   *  pointing at the Whisper section when nothing is downloaded. */
  private renderDedicatedModelSelect(host: HTMLElement) {
    if (!this.config.in_place?.stt) return;
    const current = this.config.in_place.stt.model_path ?? "";
    const currentNorm = current.replace(/\\/g, "/");
    const options = this.downloaded.length
      ? this.downloaded
          .map((p) => {
            const sel =
              currentNorm && currentNorm.endsWith(p.replace(/\\/g, "/").split("/").pop() ?? "")
                ? "selected"
                : "";
            return `<option value="${p.replace(/"/g, "&quot;")}" ${sel}>${prettyModel(p)}</option>`;
          })
          .join("")
      : `<option value="">No models downloaded yet</option>`;

    host.innerHTML = `
      <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
        <div style="width:100%;"><select id="ip-stt-dedicated-model" style="width:100%; max-width:400px;">${options}</select></div>
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
          Download models in the <b>Whisper</b> section (Transcription). A small model
          (Tiny / Base) is the right pick for snappy dictation on the dedicated server.${
            this.downloaded.length ? "" : " Download a model in the <b>Whisper</b> section first."
          }
        </span>
      </div>`;

    host.querySelector<HTMLSelectElement>("#ip-stt-dedicated-model")?.addEventListener("change", (e) => {
      const path = (e.target as HTMLSelectElement).value;
      if (path && this.config.in_place?.stt) {
        this.config.in_place.stt.model_path = path;
      }
    });

    // Fetch the downloaded-models list once, then re-render the select so it
    // shows real options (the same async-populate pattern SectionPreview uses).
    if (!this.downloadedLoaded) {
      this.downloadedLoaded = true;
      void invoke<string[]>("wizard_list_downloaded_models")
        .then((list) => {
          this.downloaded = list;
        })
        .catch(() => {
          this.downloaded = [];
        })
        .finally(() => {
          // The dedicated branch may have been navigated away from while the
          // probe was in flight; only repopulate if its host is still mounted.
          const stillThere = this.container.querySelector<HTMLElement>("#ip-stt-dedicated-model-host");
          if (stillThere && this.config.in_place?.stt) this.renderDedicatedModelSelect(stillThere);
        });
    }
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
