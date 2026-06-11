import { errText } from "../../utils/error";
import { renderField, bindFieldEvents } from "./form";
import { reembedAll } from "../../services/ipc";
import { showToast } from "../../utils/toast";

/**
 * Semantic search settings: enable indexing, pick the embedding model folder,
 * and adapt Phoneme to embedding models other than the bundled all-MiniLM
 * (pooling, max tokens, whether the model takes `token_type_ids`, and the
 * query/passage prefixes E5/BGE expect). A "Re-embed library" action re-indexes
 * everything with the current model — run it after switching models.
 */
export class SectionSemantic {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    container: HTMLElement,
    private config: any,
  ) {
    if (!this.config.semantic_search) this.config.semantic_search = {};
    this.render(container);
  }

  private render(container: HTMLElement) {
    const s = this.config.semantic_search;
    container.innerHTML = `
      <div class="settings-section">
        <h3>Semantic Search</h3>

        <div class="settings-field">
          <label>Enable semantic search</label>
          <div>${renderField({ key: "semantic_search.enabled", label: "", kind: "checkbox" }, s.enabled)}</div>
          <span>Index transcripts with a local embedding model so you can search by meaning, not just keywords.
            The model loads into memory while enabled.</span>
        </div>

        <div class="settings-field">
          <label>Embedding model folder</label>
          <div>
            ${renderField({ key: "semantic_search.model_dir", label: "", kind: "text", placeholder: "…/models/all-MiniLM-L6-v2" }, s.model_dir)}
            <button class="inline-button" id="pick-model-dir">Browse…</button>
          </div>
          <span>A folder containing <code>model.onnx</code> and <code>tokenizer.json</code>. The bundled default is
            all-MiniLM-L6-v2 (384-dim). After changing this, click Save, then Re-embed the library below.</span>
        </div>

        <details class="settings-advanced">
          <summary>
            <svg class="settings-advanced-chev" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true">
              <path d="M9 6l6 6-6 6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
            Advanced — model compatibility
          </summary>
          <span style="display:block; font-size:11px; color:var(--fg-faded); margin:4px 0 10px;">
            Defaults match all-MiniLM. Adjust these to use other ONNX sentence-transformers (E5, BGE, GTE, MPNet…).
          </span>

          <div class="settings-field">
            <label>Max tokens</label>
            <div>${renderField({ key: "semantic_search.max_tokens", label: "", kind: "number" }, s.max_tokens ?? 256)}</div>
            <span>Input length cap before truncation. all-MiniLM was trained at 256.</span>
          </div>

          <div class="settings-field">
            <label>Pooling</label>
            <div>${renderField(
              {
                key: "semantic_search.pooling",
                label: "",
                kind: "select",
                options: [
                  { value: "mean", label: "Mean (MiniLM / MPNet / E5 / BGE)" },
                  { value: "cls", label: "CLS token" },
                ],
              },
              s.pooling ?? "mean",
            )}</div>
            <span>How per-token vectors are reduced to one sentence vector.</span>
          </div>

          <div class="settings-field">
            <label>Model uses token_type_ids</label>
            <div>${renderField({ key: "semantic_search.token_type_ids", label: "", kind: "checkbox" }, s.token_type_ids ?? true)}</div>
            <span>On for BERT-family models (MiniLM, MPNet). Turn OFF for models that don't take this input (some E5
              exports) — they error if fed one.</span>
          </div>

          <div class="settings-field">
            <label>Query prefix</label>
            <div>${renderField({ key: "semantic_search.query_prefix", label: "", kind: "text", placeholder: "e.g. query: " }, s.query_prefix ?? "")}</div>
            <span>Prepended to a search query before embedding. E5 wants <code>query: </code>; leave empty for all-MiniLM.</span>
          </div>

          <div class="settings-field">
            <label>Passage prefix</label>
            <div>${renderField({ key: "semantic_search.passage_prefix", label: "", kind: "text", placeholder: "e.g. passage: " }, s.passage_prefix ?? "")}</div>
            <span>Prepended to each stored transcript before embedding. E5 wants <code>passage: </code>; leave empty for all-MiniLM.</span>
          </div>
        </details>

        <div class="settings-field">
          <label>Re-embed library</label>
          <div>
            <button class="inline-button" id="reembed-all">↻ Re-embed all recordings…</button>
            <span id="reembed-status" style="font-size:11px; color: var(--fg-muted);"></span>
          </div>
          <span>Clears all embeddings and re-indexes every recording with the current model. Run this after changing
            the model (Save first). Runs in the background.</span>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    container.querySelector("#pick-model-dir")?.addEventListener("click", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const dir = await open({ directory: true, multiple: false });
      if (typeof dir === "string") {
        const input = container.querySelector<HTMLInputElement>(`[data-key="semantic_search.model_dir"]`);
        if (input) input.value = dir;
        this.config.semantic_search.model_dir = dir;
      }
    });

    container.querySelector("#reembed-all")?.addEventListener("click", async () => {
      if (
        !confirm(
          "Re-embed the entire library with the current model?\n\nThis clears all existing embeddings and re-indexes " +
            "every recording in the background. Recommended after changing the embedding model. Make sure you've Saved " +
            "your settings first.",
        )
      ) {
        return;
      }
      const statusEl = container.querySelector<HTMLElement>("#reembed-status");
      try {
        await reembedAll();
        if (statusEl) statusEl.textContent = "Re-embedding started — running in the background.";
        showToast("Re-embedding the library in the background…", "info");
      } catch (e) {
        showToast(`Re-embed failed: ${errText(e)}`, "error");
        if (statusEl) statusEl.textContent = "";
      }
    });
  }
}
