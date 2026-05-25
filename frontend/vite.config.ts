/// <reference types="vitest" />
import { defineConfig } from "vite";

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
  },
  test: {
    // SettingsView tests instantiate the component and touch `document` /
    // `window.confirm`, which don't exist in Vitest's default node environment.
    environment: "jsdom",
  },
});
