import React from "react";
import ReactDOM from "react-dom/client";

// Self-hosted fonts (bundled by Vite → no external requests, CSP-safe).
// One UI face + one mono face, same as dotori's launcher.
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";

import "./styles/app.css";
import { App } from "./App";
import { initTheme } from "./theme";

// The CSS only reads data-theme — resolve it before first paint so there is
// no theme flash.
initTheme();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
