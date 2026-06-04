/**
 * Audio-file import helpers shared by the "Import audio…" button (file dialog)
 * and the window drag-drop handler. Both ultimately call the `import_recording`
 * Tauri command, which hands the file to the daemon for decode + transcription.
 */

import { importRecording, IMPORT_AUDIO_EXTENSIONS } from "../services/ipc";
import { showToast } from "./toast";

/** Basename of a path (handles both `/` and `\` separators). */
function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function hasSupportedExtension(path: string): boolean {
  const dot = path.lastIndexOf(".");
  if (dot < 0) return false;
  const ext = path.slice(dot + 1).toLowerCase();
  return (IMPORT_AUDIO_EXTENSIONS as readonly string[]).includes(ext);
}

/**
 * Import a list of file paths. Skips files with unsupported extensions and
 * reports per-file success/failure via toasts. Returns the number imported.
 */
export async function importAudioPaths(paths: string[]): Promise<number> {
  let imported = 0;
  for (const path of paths) {
    if (!hasSupportedExtension(path)) {
      showToast(`Skipped (unsupported format): ${basename(path)}`, "warning");
      continue;
    }
    try {
      await importRecording(path);
      showToast(`Importing ${basename(path)}…`, "success");
      imported++;
    } catch (e) {
      showToast(`Import failed for ${basename(path)}: ${e}`, "error");
    }
  }
  return imported;
}

/**
 * Open a file-picker filtered to audio files and import the selection.
 * Uses the `@tauri-apps/plugin-dialog` open dialog.
 */
export async function pickAndImportAudio(): Promise<void> {
  let selected: string | string[] | null;
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    selected = await open({
      multiple: true,
      directory: false,
      title: "Import audio file",
      filters: [
        {
          name: "Audio",
          extensions: [...IMPORT_AUDIO_EXTENSIONS],
        },
      ],
    });
  } catch (e) {
    showToast(`Could not open file dialog: ${e}`, "error");
    return;
  }
  if (selected == null) return; // user cancelled
  const paths = Array.isArray(selected) ? selected : [selected];
  await importAudioPaths(paths);
}
