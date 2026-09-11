import React, { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import RootErrorBoundary from "./components/RootErrorBoundary";
import "./App.css";
import { emit } from "@tauri-apps/api/event";
import { log, toErrorFields, initLogging } from "./repositories";
import { installWindowAppearance } from "./workflows/window-appearance";
import { installMediaUseBoundary } from "./media-use";
import { presentEscapedFailure, recordInterfaceFailure } from "./utils/failureSurface";
import { closeComparisonAfterMainRendererFailure } from "./state/comparison-store";

const App = lazy(() => import("./App"));
const PreviewWindow = lazy(() => import("./windows/PreviewWindow"));
const ComparisonWindow = lazy(() => import("./windows/ComparisonWindow"));
const ComparisonImageWindow = lazy(() => import("./windows/ComparisonImageWindow"));
const IdentifyWindow = lazy(() => import("./windows/IdentifyWindow"));
const ViewerWindow = lazy(() => import("./windows/ViewerWindow"));

// One entry serves every window; lazy routes keep unrelated window code out of
// each renderer's initial load.
const params = new URLSearchParams(window.location.search);
const view = params.get("view");
const slice = Number.parseInt(params.get("slice") ?? "0", 10) || 0;

// Learn the core's debug gate as early as possible. Fire-and-forget: emit()
// already works before this resolves (defaulting to the dev-build gate).
void initLogging();

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
let presentationFailureHandled = false;

function recoverComparisonPresentation(): void {
  if (presentationFailureHandled) return;
  presentationFailureHandled = true;
  if (view === "comparison") {
    void emit("comparison://display-failed", { slice });
  } else if (view === null) {
    closeComparisonAfterMainRendererFailure();
  }
}

window.addEventListener("error", (event) => {
  log.error("uncaught error", {
    ...toErrorFields(event.error ?? event.message),
    source: event.filename,
    line: event.lineno,
    column: event.colno,
  });
  const presentation = "This window stopped unexpectedly. Reload it before continuing.";
  recordInterfaceFailure(presentation);
  recoverComparisonPresentation();
  presentEscapedFailure(presentation);
});

window.addEventListener("unhandledrejection", (event) => {
  log.error("unhandled promise rejection", toErrorFields(event.reason));
  const presentation = "This window could not finish an action. Reload it before continuing.";
  recordInterfaceFailure(presentation);
  recoverComparisonPresentation();
  presentEscapedFailure(presentation);
});

void Promise.all([installMediaUseBoundary(), installWindowAppearance()])
  .then(() => {
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <RootErrorBoundary onFailure={recoverComparisonPresentation}>
          <Suspense fallback={null}>
            {view === "preview" ? (
              <PreviewWindow />
            ) : view === "comparison" ? (
              <ComparisonWindow slice={slice} />
            ) : view === "comparison-image" ? (
              <ComparisonImageWindow />
            ) : view === "identify" ? (
              <IdentifyWindow number={slice} />
            ) : view === "viewer" ? (
              <ViewerWindow />
            ) : (
              <App />
            )}
          </Suspense>
        </RootErrorBoundary>
      </React.StrictMode>,
    );
  })
  .catch((error) => {
    log.error("media ownership bootstrap failed", toErrorFields(error));
    const presentation = "This window could not start safely. Reload it before continuing.";
    recordInterfaceFailure(presentation);
    presentEscapedFailure(presentation);
  });
