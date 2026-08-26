import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import PreviewWindow from "./windows/PreviewWindow";
import ComparisonWindow from "./windows/ComparisonWindow";
import IdentifyWindow from "./windows/IdentifyWindow";
import RootErrorBoundary from "./components/RootErrorBoundary";
import "./App.css";
import { log, toErrorFields, initLogging, loadAppData } from "./repositories";
import { applyTheme, applyUiFont, watchSystemTheme } from "./utils/theme";

// Learn the core's debug gate as early as possible. Fire-and-forget: emit()
// already works before this resolves (defaulting to the dev-build gate).
void initLogging();

// Theme before first meaningful paint, in EVERY window (one bundle serves
// all); the OS-preference listener keeps "system" live.
watchSystemTheme();
void loadAppData()
  .then((data) => {
    const config = data.config as { theme?: unknown; uiFontFamily?: unknown } | null;
    applyTheme(config?.theme);
    applyUiFont(config?.uiFontFamily);
  })
  .catch((error) => {
    log.warn("startup appearance load failed", toErrorFields(error));
    applyTheme("system");
  });

// The webview's default context menu (Look Up, Translate, Search with
// Google, Inspect Element…) belongs to a web page, not a desktop app —
// suppressed everywhere except text-entry surfaces, where the native menu
// carries Paste. Registered here so EVERY window gets it.
window.addEventListener("contextmenu", (event) => {
  const target = event.target as HTMLElement | null;
  if (!target?.closest("input, textarea, [contenteditable='true']")) {
    event.preventDefault();
  }
});

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
    <RootErrorBoundary>
      {view === "preview" ? (
        <PreviewWindow />
      ) : view === "comparison" ? (
        <ComparisonWindow slice={slice} />
      ) : view === "identify" ? (
        <IdentifyWindow number={slice} />
      ) : (
        <App />
      )}
    </RootErrorBoundary>
  </React.StrictMode>,
);
