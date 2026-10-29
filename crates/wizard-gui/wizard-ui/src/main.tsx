import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { LogWindow } from "./logs/LogWindow";
import "./styles.css";
import { initializeTheme } from "./settings/theme";

initializeTheme();
const isLogWindow = window.location.hash === "#logs";
if (navigator.platform.startsWith("Mac") && !isLogWindow) {
  document.documentElement.dataset.macWindow = "";
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <div aria-hidden="true" data-tauri-drag-region className="fixed top-0 left-0 z-50 hidden h-8 w-full mac-window:block lg:w-64" />
    {isLogWindow ? <LogWindow /> : <App />}
  </React.StrictMode>,
);
