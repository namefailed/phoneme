// Tiny declarative form helpers.

export type FieldKind = "text" | "number" | "checkbox" | "select" | "textarea";

export type Field = {
  key: string; // dotted path e.g. "whisper.timeout_secs"
  label: string;
  kind: FieldKind;
  help?: string;
  type?: string;
  list?: string;
  options?: { value: string; label: string }[]; // for "select"
};

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function getByPath(obj: any, path: string): any {
  return path.split(".").reduce((o, k) => (o == null ? undefined : o[k]), obj);
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function setByPath(obj: any, path: string, value: any): void {
  const parts = path.split(".");
  const last = parts.pop()!;
  const target = parts.reduce((o, k) => o[k], obj);
  target[last] = value;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function renderField(field: Field, value: any): string {
  switch (field.kind) {
    case "text":
      return `<input type="${field.type || "text"}" data-key="${field.key}" ${field.list ? `list="${field.list}"` : ""} value="${escapeAttr(
        String(value ?? ""),
      )}" />`;
    case "number":
      return `<input type="number" data-key="${field.key}" value="${value ?? 0}" />`;
    case "checkbox":
      return `<input type="checkbox" data-key="${field.key}" ${value ? "checked" : ""} />`;
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
      return `<textarea data-key="${field.key}" rows="8" style="resize: vertical; min-height: 140px; font-size: 13px; padding: 8px;">${escapeHtml(
        value as string,
      )}</textarea>`;
  }
}

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
