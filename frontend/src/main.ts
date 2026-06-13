/**
 * Main-window entry point (loaded by index.html via Vite). It pulls in every
 * global stylesheet — theme variables, reset, toasts, and the shared modal /
 * model-picker / tag-manager styles, which must be global because components
 * render into the light DOM — then boots the {@link App} controller into
 * `#app`. The separate live-preview overlay window has its own entry point
 * (`overlay.ts`); nothing here runs in that window.
 */
import "./styles/theme.css";
import "./styles/reset.css";
import "./styles/toast.css";
import { App } from "./App";
import "./components/modal.css";
import "./components/model-picker.css";
import "./components/tag-manager.css";

const root = document.getElementById("app");
if (root) {
  new App(root);
}
