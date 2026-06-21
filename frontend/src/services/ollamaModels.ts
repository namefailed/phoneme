/**
 * Local-Ollama model management — list / pull / delete.
 *
 * These wrap the tray's `ollama_*` Tauri commands (src-tauri/src/commands/wizard.rs),
 * which talk to the local Ollama HTTP API directly (the same plane as the
 * first-run wizard's model pull). They power the "Manage local models" surface
 * reachable from the Models picker and Settings → Post-Processing — installed
 * models with sizes, a delete button, and a streaming pull. A pull's progress
 * arrives on the `ollama_pull_progress` Tauri event (see {@link OllamaPullProgress}),
 * not as a return value, so callers subscribe before invoking.
 *
 * Distinct from {@link listLlmModels}-style live fetches: this is *management*
 * (mutating the local install), not just reading what the configured connection
 * offers.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** One installed Ollama model, mirroring the daemon-side `OllamaInstalledModel`.
 *  `size` is on-disk bytes (null on an older Ollama that omits it); `modified_at`
 *  is Ollama's ISO timestamp string (null when absent). */
export type OllamaInstalledModel = {
  name: string;
  size: number | null;
  modified_at: string | null;
};

/** One NDJSON progress object from a pull, mirroring the `ollama_pull_progress`
 *  Tauri event payload. `completed`/`total` are byte counts during the download
 *  layers (null for the metadata phases). */
export type OllamaPullProgress = {
  status: string;
  completed: number | null;
  total: number | null;
};

/** List the models installed in the local Ollama (`GET /api/tags`), sorted by
 *  name. Rejects with an `ollama_unreachable` error when Ollama isn't running. */
export async function listInstalledOllamaModels(): Promise<OllamaInstalledModel[]> {
  return await tauriInvoke<OllamaInstalledModel[]>("ollama_list_installed");
}

/** Delete an installed model from the local Ollama (`DELETE /api/delete`),
 *  freeing its disk. Rejects with `not_found` for an unknown model. */
export async function deleteOllamaModel(model: string): Promise<void> {
  await tauriInvoke("ollama_delete_model", { model });
}

/**
 * Pull a model into the local Ollama (`POST /api/pull`), reporting progress via
 * `onProgress` as the download streams. Resolves when the pull completes; rejects
 * on a network/HTTP error or an Ollama-reported pull failure (e.g. unknown model).
 *
 * Subscribes to the `ollama_pull_progress` Tauri event for the duration of the
 * call and unsubscribes when it settles, so concurrent pulls never leak listeners.
 */
export async function pullOllamaModel(
  model: string,
  onProgress?: (p: OllamaPullProgress) => void,
): Promise<void> {
  const unlisten = onProgress
    ? await listen<OllamaPullProgress>("ollama_pull_progress", (e) => onProgress(e.payload))
    : null;
  try {
    await tauriInvoke("ollama_pull_model", { model });
  } finally {
    unlisten?.();
  }
}

/** Format a byte count as a compact human size (e.g. "2.0 GB"). Null/0 → "—". */
export function formatBytes(bytes: number | null | undefined): string {
  if (!bytes || bytes <= 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  // One decimal for MB and up; whole numbers for B/KB.
  return `${i >= 2 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}
