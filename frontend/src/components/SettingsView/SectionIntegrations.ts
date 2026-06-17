import { renderField, bindFieldEvents } from "./form";

/**
 * Settings → Integrations: the two opt-in automation surfaces that live OUTSIDE
 * the daemon's named pipe.
 *
 *  - **REST / SSE bridge** (`[rest_api]`): the `phoneme-rest` binary exposes the
 *    daemon over loopback HTTP for scripts and other languages. Config-gated
 *    (`rest_api.enabled` / `rest_api.port`) — this section is where the user
 *    turns it on and picks the port; the binary still has to be launched.
 *  - **MCP server** (`phoneme-mcp`): a Model Context Protocol stdio server that
 *    lets MCP-aware AI clients (Claude Desktop, etc.) drive recording and search.
 *    It has no config of its own — it's enabled by adding the binary to the
 *    client's MCP config — so this is an info card, not editable fields.
 *
 * Plain section class on the form.ts binding (same shape as the other sections).
 */
export class SectionIntegrations {
  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    // Seed `[rest_api]` so the toggle/port can bind (setByPath throws on a
    // missing parent). Defaults mirror RestApiConfig: off, port 3737.
    const r = config.rest_api ?? (config.rest_api = {});
    if (typeof r.enabled !== "boolean") r.enabled = false;
    if (typeof r.port !== "number") r.port = 3737;

    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Integrations (REST &amp; MCP)</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.4;">
          Two opt-in ways to drive Phoneme from outside the app. Both talk to the
          same local daemon; both are off until you turn them on.
        </p>

        <div class="settings-field">
          <label>REST API bridge</label>
          <div>${renderField(
            { key: "rest_api.enabled", label: "", kind: "checkbox" },
            this.config.rest_api.enabled ?? false,
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            Allow the <code>phoneme-rest</code> binary to run. It serves the daemon over
            <b>loopback-only</b> HTTP + SSE (<code>127.0.0.1</code>, never <code>0.0.0.0</code>) for scripts and
            other languages. Off by default — when off, <code>phoneme-rest</code> exits with a clear
            message and no HTTP port is ever opened. Enabling it here just permits it; you still
            launch the <code>phoneme-rest</code> binary yourself.
          </span>
        </div>

        <div class="settings-field">
          <label>REST API port</label>
          <div>${renderField(
            { key: "rest_api.port", label: "", kind: "number" },
            this.config.rest_api.port ?? 3737,
          )}</div>
          <span style="font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;">
            TCP port the bridge binds on <code>127.0.0.1</code>. Default <b>3737</b>.
          </span>
        </div>

        <div class="settings-field conn-field">
          <label>MCP server</label>
          <div style="display: block; font-size: 0.8571rem; color: var(--fg-muted); line-height: 1.5; max-width: 760px;">
            <p style="margin: 0 0 8px;">
              <code>phoneme-mcp</code> is a <a href="https://modelcontextprotocol.io" target="_blank" rel="noreferrer">Model Context Protocol</a>
              stdio server. Point an MCP-aware client (Claude Desktop, etc.) at the
              <code>phoneme-mcp</code> binary and it can drive Phoneme through the running daemon —
              no extra config in Phoneme, so there's nothing to switch on here.
            </p>
            <p style="margin: 0 0 6px;">It exposes five tools:</p>
            <ul style="margin: 0 0 8px; padding-left: 18px;">
              <li><code>start_recording</code> · <code>stop_recording</code> — control capture</li>
              <li><code>get_transcript</code> — fetch one recording's transcript</li>
              <li><code>search_recordings</code> — semantic + lexical search over the library</li>
              <li><code>list_recent</code> — list the latest recordings</li>
            </ul>
            <p style="margin: 0; color: var(--fg-faded); font-size: 0.7857rem;">
              Add it to your client's MCP config as a stdio server whose command is the
              <code>phoneme-mcp</code> executable (next to <code>phoneme.exe</code>). See
              <code>docs/developer-guide/mcp_server.md</code> for a ready-to-paste example.
            </p>
          </div>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);
  }
}
