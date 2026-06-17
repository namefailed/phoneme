/// <reference types="vitest" />
import { defineConfig } from "vite";
import { fileURLToPath, URL } from "node:url";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  // CodeMirror's extension system relies on `instanceof` against a single
  // `@codemirror/state` module. `@replit/codemirror-vim` re-imports state/view,
  // and Vite's dep pre-bundler can otherwise optimize them into a SECOND copy —
  // which makes `EditorState.create` throw "Unrecognized extension value" and
  // leaves both the transcript and notes editors blank. Deduping the resolution
  // and pre-bundling the whole CodeMirror set in one pass guarantees one copy.
  resolve: {
    dedupe: ["@codemirror/state", "@codemirror/view"],
  },
  optimizeDeps: {
    include: [
      "@codemirror/state",
      "@codemirror/view",
      "@codemirror/commands",
      "@codemirror/language",
      "@replit/codemirror-vim",
      "codemirror",
    ],
  },
  build: {
    target: "esnext",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    // Multi-page build: the app shell (index.html) AND the two standalone
    // always-on-top overlay windows — the system-wide live-preview caption
    // overlay (overlay.html, src-tauri/src/overlay.rs) and the minimal recording
    // indicator pill (indicator.html, src-tauri/src/indicator.rs). Without
    // listing each HTML as a Rollup input it would be omitted from `dist/`, so
    // that window would 404. Each HTML entry pulls in its own module graph.
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        overlay: fileURLToPath(new URL("./overlay.html", import.meta.url)),
        indicator: fileURLToPath(new URL("./indicator.html", import.meta.url)),
      },
    },
  },
  test: {
    // SettingsView tests instantiate the component and touch `document` /
    // `window.confirm`, which don't exist in Vitest's default node environment.
    environment: "jsdom",
  },
});
