import "./styles/theme.css";
import "./styles/reset.css";
import { App } from "./App";

const root = document.getElementById("app");
if (root) {
  new App(root);
}
