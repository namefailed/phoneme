// Pure key-event → combo-string parsing for the wizard's hotkey-capture step.
// The component keeps the listener wiring, capture state, and config writes;
// this is just the stateless "turn this KeyboardEvent into Ctrl+Shift+K" bit.

/** Bare modifier / Escape keys that are never a combo on their own. */
const IGNORE_HOTKEY_KEYS = ["Control", "Shift", "Alt", "Meta", "Escape"];

/** Build a "Ctrl+Shift+K"-style combo from a keydown event, or null when the key
 *  is a bare modifier (the caller should ignore it and keep listening). */
export function eventToHotkeyCombo(e: KeyboardEvent): string | null {
  const modifiers: string[] = [];
  if (e.ctrlKey) modifiers.push("Ctrl");
  if (e.shiftKey) modifiers.push("Shift");
  if (e.altKey) modifiers.push("Alt");
  if (e.metaKey) modifiers.push("Super");

  if (IGNORE_HOTKEY_KEYS.includes(e.key)) return null;

  const keyName = e.code.startsWith("Key") ? e.code.replace("Key", "") :
          e.code.startsWith("Digit") ? e.code.replace("Digit", "") :
          e.key.length === 1 ? e.key.toUpperCase() : e.key;

  return [...modifiers, keyName].join("+");
}
