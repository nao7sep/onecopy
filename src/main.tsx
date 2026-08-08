import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import PreviewWindow from "./windows/PreviewWindow";
import ComparisonWindow from "./windows/ComparisonWindow";
import "./App.css";
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

// One bundle serves every window; the `view` query parameter routes.
const params = new URLSearchParams(window.location.search);
const view = params.get("view");
const slice = Number.parseInt(params.get("slice") ?? "0", 10) || 0;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {view === "preview" ? (
      <PreviewWindow />
    ) : view === "comparison" ? (
      <ComparisonWindow slice={slice} />
    ) : (
      <App />
    )}
  </React.StrictMode>,
);
