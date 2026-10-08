import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { LogWindow } from "./logs/LogWindow";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {window.location.hash === "#logs" ? <LogWindow /> : <App />}
  </React.StrictMode>,
);
