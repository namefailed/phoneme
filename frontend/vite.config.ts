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
  build: {
    target: "esnext",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    // Multi-page build: the app shell (index.html) AND the standalone
    // system-wide live-preview overlay window (overlay.html, loaded by the
    // Tauri WebviewWindow created in src-tauri/src/overlay.rs). Without listing
    // overlay.html as a Rollup input it would be omitted from `dist/`, so the
    // overlay window would 404. Each HTML entry pulls in its own module graph.
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        overlay: fileURLToPath(new URL("./overlay.html", import.meta.url)),
      },
    },
  },
  test: {
    // SettingsView tests instantiate the component and touch `document` /
    // `window.confirm`, which don't exist in Vitest's default node environment.
    environment: "jsdom",
  },
});
