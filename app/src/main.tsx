import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/bundle.css";
import "./styles/app.css";
import "./styles/base.css";
import "./styles/notebook.css";
import { App } from "./App";

// The design system's dark theme follows the system.
const dark = window.matchMedia("(prefers-color-scheme: dark)");
const theme = () => (document.documentElement.dataset.theme = dark.matches ? "dark" : "light");
theme();
dark.addEventListener("change", theme);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
