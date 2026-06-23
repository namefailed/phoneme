/** Friendly label for a downloaded whisper model filename (mirrors the Live
 *  Preview section's local dropdown, so the dedicated dictation picker reads
 *  the same way). */
export function prettyModel(path: string): string {
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
