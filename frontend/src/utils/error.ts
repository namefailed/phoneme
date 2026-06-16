/**
 * Helpers for the structured error a Tauri command rejects with.
 *
 * Commands return `{ kind, message }` (Rust `CommandError`) instead
 * of a flattened string, so the WebView can branch on `kind`. These helpers
 * normalize any caught value — structured command error, plain `Error`, or a
 * bare string — to displayable text, and expose the `kind` when present.
 */

/** The structured error shape a Tauri command rejects with. */
export interface IpcError {
  kind: string;
  message: string;
}

/** Type guard for the structured `{ kind, message }` command error. */
export function isIpcError(e: unknown): e is IpcError {
  return (
    typeof e === "object" &&
    e !== null &&
    "kind" in e &&
    "message" in e &&
    typeof (e as { message: unknown }).message === "string"
  );
}

/**
 * Human-readable text for any caught value: the structured `message` for a
 * command error, `Error.message` for a JS error, otherwise `String(e)`.
 */
export function errText(e: unknown): string {
  if (isIpcError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** The `kind` of a structured command error, or `undefined` for other values. */
export function errKind(e: unknown): string | undefined {
  return isIpcError(e) ? e.kind : undefined;
}
