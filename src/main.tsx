import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { log, toErrorFields, initLogging } from "./repositories";

// Learn the core's debug gate as early as possible. Fire-and-forget: emit()
// already works before this resolves (defaulting to the dev-build gate).
void initLogging();

// Global last-resort handlers — catch anything that slips past React's error
// handling and record it before the page can tear down.
window.addEventListener("error", (event) => {
  log.error("uncaught error", {
    ...toErrorFields(event.error ?? event.message),
    source: event.filename,
    line: event.lineno,
    column: event.colno,
  });
});

window.addEventListener("unhandledrejection", (event) => {
  log.error("unhandled promise rejection", toErrorFields(event.reason));
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
