// Native behavior of the main window. App renders the shell; this hook owns
// the physical window/webview lifetime, content minimum, and persisted zoom.

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import {
  getCurrentWindow,
  LogicalSize,
} from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { LoadedAppData } from "../repositories";
import { reportWindowCall } from "../repositories";
import {
  flushStatePatchesForShutdown,
  reportStatePatchFailure,
  resumeStatePatchesAfterFailedShutdown,
  retainStatePatch,
} from "../state/app-store";
import { installActivityPings } from "../state/derived-work-store";
import { usePreviewStore } from "../state/preview-store";
import { hasOpenModal } from "../utils/modalStack";
import { isComposingEvent } from "./useComposing";
import { isEditableTarget, shadowsMacTextEditing } from "../utils/shortcuts";
import { computeMinWindowHeight, computeMinWindowWidth } from "../utils/windowSizing";
import { installDisplayZoneReconciliation } from "../workflows/display-zone";
import {
  ZOOM_DEFAULT,
  isZoomIn,
  isZoomOut,
  isZoomReset,
  stepZoomIn,
  stepZoomOut,
} from "../utils/zoom";

interface MainWindowLifecycleOptions {
  appData: LoadedAppData;
  splitOpen: boolean;
}

export function useMainWindowLifecycle({
  appData,
  splitOpen,
}: MainWindowLifecycleOptions) {
  // The derived-work coordinator's view of the user: throttled input pings.
  useEffect(() => installActivityPings(window), []);
  useEffect(() => installDisplayZoneReconciliation(), []);

  // A newly opened preview window asks for the exact message already owned
  // by the preview store. Rebuilding it from selection would lose one-shot
  // presentation intent such as image zoom or a video snapshot seek.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen("preview://ready", () => {
      const current = usePreviewStore.getState().current;
      if (current !== null) void emit("preview://show", current);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // On Windows setMinSize restores a maximized window. Remember the current
  // content floor and apply it on the first resize after the window is normal.
  const pendingMinSize = useRef<LogicalSize | null>(null);
  useEffect(() => {
    const size = new LogicalSize(computeMinWindowWidth(splitOpen), computeMinWindowHeight());
    const appWindow = getCurrentWindow();
    void (async () => {
      try {
        if (await appWindow.isMaximized()) {
          pendingMinSize.current = size;
          return;
        }
        pendingMinSize.current = null;
        await appWindow.setMinSize(size);
      } catch (error) {
        reportWindowCall("setMinSize")(error);
      }
    })();
  }, [splitOpen]);

  useEffect(() => {
    const appWindow = getCurrentWindow();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void appWindow.onResized(() => {
      void (async () => {
        const size = pendingMinSize.current;
        if (size === null || await appWindow.isMaximized()) return;
        pendingMinSize.current = null;
        await appWindow.setMinSize(size);
      })().catch(reportWindowCall("apply deferred main window minimum"));
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(reportWindowCall("listen for main window resize"));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // The configured main window starts hidden so WebView2 cannot flash a white
  // frame. Once the React shell owns the lifecycle, reveal it without changing
  // the operating system's placement.
  useEffect(() => {
    const appWindow = getCurrentWindow();
    void appWindow.show()
      .then(() => appWindow.setFocus())
      .catch(reportWindowCall("show main window"));
  }, []);

  // Main-window close is also the application quit edge. The Rust menu routes
  // Cmd/Ctrl+Q here so ordinary shutdown quiescence has one owner.
  const closeHandlerStarted = useRef(false);
  useEffect(() => {
    if (closeHandlerStarted.current) return;
    closeHandlerStarted.current = true;
    const appWindow = getCurrentWindow();
    let closing = false;
    void appWindow.onCloseRequested(async (event) => {
      event.preventDefault();
      if (closing) return;
      closing = true;
      try {
        await flushStatePatchesForShutdown();
      } catch (error) {
        reportStatePatchFailure(error);
      }
      try {
        await invoke("request_app_exit");
      } catch (error) {
        closing = false;
        resumeStatePatchesAfterFailedShutdown();
        reportWindowCall("request application exit")(error);
      }
    }).catch(reportWindowCall("listen for main window close"));
  }, []);

  const zoomRef = useRef(ZOOM_DEFAULT);
  const [zoomLevel, setZoomLevel] = useState(ZOOM_DEFAULT);
  const applyZoom = useCallback((next: number) => {
    zoomRef.current = next;
    setZoomLevel(next);
    void getCurrentWebview().setZoom(next).catch(reportWindowCall("setZoom"));
    retainStatePatch({ zoomLevel: next });
  }, []);

  useEffect(() => {
    const stored = appData.state?.zoomLevel;
    const level = typeof stored === "number" ? stored : ZOOM_DEFAULT;
    zoomRef.current = level;
    setZoomLevel(level);
    if (level !== ZOOM_DEFAULT) {
      void getCurrentWebview().setZoom(level).catch(reportWindowCall("setZoom"));
    }
  }, [appData]);

  useEffect(() => {
    const onZoomKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || isComposingEvent(event) || hasOpenModal()
        || (isEditableTarget(event.target) && shadowsMacTextEditing(event))) return;
      const zoomIn = isZoomIn(event);
      const zoomOut = isZoomOut(event);
      const zoomReset = isZoomReset(event);
      if (!zoomIn && !zoomOut && !zoomReset) return;
      event.preventDefault();
      applyZoom(
        zoomReset
          ? ZOOM_DEFAULT
          : zoomIn
            ? stepZoomIn(zoomRef.current)
            : stepZoomOut(zoomRef.current),
      );
    };
    window.addEventListener("keydown", onZoomKey);
    return () => window.removeEventListener("keydown", onZoomKey);
  }, [applyZoom]);

  return {
    zoomLevel,
    zoomIn: () => applyZoom(stepZoomIn(zoomRef.current)),
    zoomOut: () => applyZoom(stepZoomOut(zoomRef.current)),
  };
}
