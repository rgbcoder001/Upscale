import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { useQueue } from "./queue/store";

// Dev builds expose the store, so the UI can be exercised in a plain browser without the core
if (import.meta.env.DEV) Object.assign(window, { __queue: useQueue });

// `pnpm dev` + /?demo=rip: the Rip sheet with a sample disc (src/rip/demo.ts)
if (import.meta.env.DEV && new URLSearchParams(location.search).get("demo") === "rip") {
  import("./rip/demo").then(({ enableDemo, demoDisc }) => {
    enableDemo();
    useQueue.getState().setPendingDisc(demoDisc);
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
