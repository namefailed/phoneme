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
