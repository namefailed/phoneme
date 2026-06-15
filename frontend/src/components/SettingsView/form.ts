// Tiny declarative form helpers — the data-binding layer every plain (non-Lit)
// Settings section is built on. A section renders inputs with
// `renderField({ key: "whisper.timeout_secs", … })` and calls
// `bindFieldEvents(container, config)` once; from then on each input writes
// its value to that dotted path in the SHARED config object on every
// input/change, and the Settings Save button persists the object. No
// validation, no events out — the config object is the form state.

/** The supported input flavors. */
export type FieldKind = "text" | "number" | "checkbox" | "select" | "textarea";

/** One field's declaration. `key` is the dotted path into the config object
 *  (it becomes the element's `data-key` and must point at a parent object
 *  that already exists — sections seed missing config tables before
 *  rendering). `kind` picks the markup; `type`/`list`/`placeholder` pass
 *  through to text inputs; `options` populates a select. `label`/`help` are
 *  carried for callers that lay the field out themselves. */
export type Field = {
  key: string; // dotted path e.g. "whisper.timeout_secs"
  label: string;
  kind: FieldKind;
  help?: string;
  type?: string;
  list?: string;
  placeholder?: string;
  options?: { value: string; label: string }[]; // for "select"
};

/** Read a dotted path from a nested object (`undefined` past any null/missing
 *  link, never a throw). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function getByPath(obj: any, path: string): any {
  return path.split(".").reduce((o, k) => (o == null ? undefined : o[k]), obj);
}

/** Write `value` at a dotted path, mutating `obj` in place. Unlike getByPath
 *  this THROWS if an intermediate object is missing — by design, so a typo'd
 *  field key (or an unseeded config table) fails loudly in dev instead of
 *  silently dropping the user's edit. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function setByPath(obj: any, path: string, value: any): void {
  const parts = path.split(".");
  const last = parts.pop()!;
  const target = parts.reduce((o, k) => o[k], obj);
  target[last] = value;
}

/** The HTML string for one field, pre-filled with `value` (escaped) and
 *  tagged with `data-key` for bindFieldEvents to find. Callers compose it
 *  into their section template (usually inside a `.settings-field` row). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function renderField(field: Field, value: any): string {
  switch (field.kind) {
    case "text":
      return `<input type="${field.type || "text"}" data-key="${field.key}" ${field.list ? `list="${field.list}"` : ""} ${field.placeholder ? `placeholder="${escapeAttr(field.placeholder)}"` : ""} value="${escapeAttr(
        String(value ?? ""),
      )}" />`;
    case "number":
      return `<input type="number" data-key="${field.key}" value="${value ?? 0}" />`;
    case "checkbox":
      return `<input type="checkbox" class="toggle-switch" data-key="${field.key}" ${value ? "checked" : ""} />`;
    case "select":
      return `<select data-key="${field.key}">${
        field.options
          ?.map(
            (o) =>
              `<option value="${o.value}" ${
                o.value === value ? "selected" : ""
              }>${o.label}</option>`,
          )
          .join("") ?? ""
      }</select>`;
    case "textarea":
      return `<textarea data-key="${field.key}" rows="8" style="resize: vertical; min-height: 140px; font-size: 0.9286rem; padding: 8px;">${escapeHtml(
        value as string,
      )}</textarea>`;
  }
}

/** Wire every `[data-key]` element under `root` to write its (type-coerced:
 *  checkbox→boolean, number→Number, else string) value to that path in
 *  `config` on input/change. Call ONCE per render, after the innerHTML is
 *  set. Sections that re-render must call it again (listeners die with the
 *  old DOM). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function bindFieldEvents(root: HTMLElement, config: any) {
  root.querySelectorAll<HTMLElement>("[data-key]").forEach((el) => {
    const key = el.getAttribute("data-key")!;
    const tag = el.tagName.toLowerCase();
    const handler = () => {
      const value = readEl(el);
      setByPath(config, key, value);
    };
    if (tag === "input" && (el as HTMLInputElement).type === "checkbox") {
      el.addEventListener("change", handler);
    } else {
      el.addEventListener("input", handler);
      el.addEventListener("change", handler);
    }
  });
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function readEl(el: HTMLElement): any {
  if (el.tagName === "SELECT") return (el as HTMLSelectElement).value;
  const input = el as HTMLInputElement;
  if (input.type === "checkbox") return input.checked;
  if (input.type === "number") return Number(input.value);
  return input.value;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(s: string): string {
  return escapeHtml(s).replace(/"/g, "&quot;");
}
