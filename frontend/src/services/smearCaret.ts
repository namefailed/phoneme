/**
 * Smear caret for the CodeMirror editors (transcript + notes) — the
 * smear-cursor.nvim feel: as the text caret moves, a translucent accent "smear"
 * stretches from where it was toward where it's going (a head that leads, a tail
 * that catches up), then settles back to CodeMirror's normal blinking caret at
 * rest.
 *
 * Model (mirrors smear-cursor.nvim): two points — head and tail — chase the live
 * caret each animation frame via per-point stiffness (the head follows the caret,
 * the tail follows the head, so the lag between them IS the smear). A 4-point
 * polygon spanning the tail caret → head caret is the smear shape; when they
 * converge it's just a thin bar, at which point we hand back to the native
 * (blinking) caret and stop.
 *
 * Tied to the same `interface.cursor_animation` setting as the app-wide cursor
 * glow: "off" leaves CM's caret untouched; "glide" is a short follow, "smear" the
 * plugin's defaults, "trail" a longer streak. Being opt-in, it takes precedence
 * over the OS "reduce motion" flag (set the mode to "off" to follow it). A
 * single SVG overlay + a spring rAF that only runs WHILE the caret is moving
 * (kicked by `selectionchange`), so it idles at zero cost between keystrokes.
 */

type Mode = "off" | "glide" | "smear" | "trail";

/** Per-mode follow stiffness (lerp factor per frame): head chases the caret, tail
 *  chases the head. Lower tail = more lag = a longer smear. */
const STIFF: Record<Exclude<Mode, "off">, { head: number; tail: number }> = {
  glide: { head: 0.7, tail: 0.55 },
  smear: { head: 0.6, tail: 0.45 },
  trail: { head: 0.5, tail: 0.32 },
};
const CARET_W = 1.5; // caret thickness (px)
const REST = 0.4; // settle threshold (px)

let mode: Mode = "off";
let svg: SVGSVGElement | null = null;
let poly: SVGPolygonElement | null = null;
let editor: HTMLElement | null = null; // the .cm-editor we're tracking
let raf = 0;
let caretH = 16;
const head = { x: 0, y: 0 };
const tail = { x: 0, y: 0 };
let installed = false;

function activeMode(): Exclude<Mode, "off"> | null {
  // Same rule as the cursor glow: the setting is opt-in (default "off"), so a
  // non-off value is a deliberate choice that wins over the OS "reduce motion"
  // flag. Set cursor_animation back to "off" to follow reduce-motion.
  return mode !== "off" ? mode : null;
}

function ensureSvg() {
  if (svg) return;
  const NS = "http://www.w3.org/2000/svg";
  svg = document.createElementNS(NS, "svg");
  svg.setAttribute("class", "smear-caret");
  svg.setAttribute("aria-hidden", "true");
  poly = document.createElementNS(NS, "polygon");
  svg.appendChild(poly);
  document.body.appendChild(svg);
}

/** The focused editor's primary caret as a top-point + height, or null. */
function caretRect(): { x: number; y: number; h: number } | null {
  const c = editor?.querySelector<HTMLElement>(".cm-cursor");
  if (!c) return null;
  const r = c.getBoundingClientRect();
  if (r.height === 0) return null;
  return { x: r.left, y: r.top, h: r.height };
}

function draw() {
  if (!poly) return;
  // Quad: tail-top → head-top → head-bottom → tail-bottom. Converged = a thin bar.
  poly.setAttribute(
    "points",
    `${tail.x},${tail.y} ${head.x},${head.y} ${head.x + CARET_W},${head.y + caretH} ${tail.x + CARET_W},${tail.y + caretH}`,
  );
}

function step() {
  raf = 0;
  const m = activeMode();
  if (!m || !editor) {
    stop();
    return;
  }
  const t = caretRect();
  if (t) caretH = t.h;
  const tx = t ? t.x : head.x;
  const ty = t ? t.y : head.y;
  const s = STIFF[m];
  head.x += (tx - head.x) * s.head;
  head.y += (ty - head.y) * s.head;
  tail.x += (head.x - tail.x) * s.tail;
  tail.y += (head.y - tail.y) * s.tail;
  draw();
  const moving =
    Math.hypot(head.x - tx, head.y - ty) > REST || Math.hypot(tail.x - head.x, tail.y - head.y) > REST;
  if (moving) {
    if (svg) svg.style.opacity = "1";
    editor.classList.add("smear-caret-on"); // hide the native caret mid-move
    raf = requestAnimationFrame(step);
  } else {
    // Settled — hand back to the native (blinking) caret.
    if (svg) svg.style.opacity = "0";
    editor.classList.remove("smear-caret-on");
  }
}

function kick() {
  if (activeMode() && editor && !raf) raf = requestAnimationFrame(step);
}

function startTracking(ed: HTMLElement) {
  editor = ed;
  ensureSvg();
  const t = caretRect();
  if (t) {
    head.x = tail.x = t.x;
    head.y = tail.y = t.y;
    caretH = t.h;
  }
}

function stop() {
  if (raf) {
    cancelAnimationFrame(raf);
    raf = 0;
  }
  if (svg) svg.style.opacity = "0";
  editor?.classList.remove("smear-caret-on");
  editor = null;
}

function cmEditorOf(t: EventTarget | null): HTMLElement | null {
  const el = t as HTMLElement | null;
  return el && typeof el.closest === "function" ? el.closest<HTMLElement>(".cm-editor") : null;
}

function onFocusIn(e: FocusEvent) {
  if (!activeMode()) return;
  const ed = cmEditorOf(e.target);
  if (ed) {
    startTracking(ed);
    kick();
  }
}

function onFocusOut(e: FocusEvent) {
  if (!cmEditorOf(e.target)) return;
  // Left the editor (Shift+Esc, a click away): if focus didn't land in another
  // editor, settle back to the native caret.
  requestAnimationFrame(() => {
    if (!cmEditorOf(document.activeElement)) stop();
  });
}

function onSelectionChange() {
  if (editor && cmEditorOf(document.activeElement) === editor) kick();
}

function applyConfig(cfg: unknown) {
  const raw = (cfg as { interface?: { cursor_animation?: string } } | null)?.interface?.cursor_animation;
  mode = raw === "glide" || raw === "smear" || raw === "trail" ? raw : "off";
  if (!activeMode()) stop();
}

/** Wire the smear caret once at app start (idempotent). Reads the saved mode and
 *  keeps it in sync with Settings saves. */
export function initSmearCaret() {
  if (installed) return;
  installed = true;
  document.addEventListener("focusin", onFocusIn);
  document.addEventListener("focusout", onFocusOut);
  document.addEventListener("selectionchange", onSelectionChange);
  import("@tauri-apps/api/core")
    .then(({ invoke }) => invoke("read_config").then(applyConfig))
    .catch(() => {
      /* keep default (off) */
    });
  window.addEventListener("config:saved", (e: Event) => applyConfig((e as CustomEvent).detail));
}
