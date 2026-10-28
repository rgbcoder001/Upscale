import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { useQueue } from "./queue/store";

// Dev builds expose the store, so the UI can be exercised in a plain browser without the core
if (import.meta.env.DEV) Object.assign(window, { __queue: useQueue });

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
