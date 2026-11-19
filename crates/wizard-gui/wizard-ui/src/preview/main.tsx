import { startPreviewMocks } from "./mocks";
import React from "react";
import ReactDOM from "react-dom/client";

// This separate HTML entry is never imported by the desktop app or its production bundle.
startPreviewMocks();
void Promise.all([import("./PreviewApp"), import("../settings/theme")]).then(([{ default: PreviewApp }, { initializeTheme }]) => {
  initializeTheme();
  if (!window.location.hash) window.location.hash = "/agents/claude-desktop";
  ReactDOM.createRoot(document.getElementById("root")!).render(<React.StrictMode><PreviewApp /></React.StrictMode>);
});
