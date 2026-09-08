// Native behavior of the main window. App renders the shell; this hook owns
// the physical window/webview lifetime and the persisted geometry/zoom state.

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import {
  availableMonitors,
  getCurrentWindow,
  LogicalSize,
} from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { LoadedAppData } from "../repositories";
import { reportWindowCall } from "../repositories";
import { reportStatePatchFailure, retainStatePatch, useAppStore } from "../state/app-store";
import { installActivityPings } from "../state/derived-work-store";
import { flushPreviewWindowPlacement, usePreviewStore } from "../state/preview-store";
import { hasOpenModal } from "../utils/modalStack";
import { isComposingEvent } from "./useComposing";
import {
  placementFromLegacyState,
  prepareWindowPlacement,
  type WindowPlacementController,
} from "../utils/windowBounds";
import { computeMinWindowHeight, computeMinWindowWidth } from "../utils/windowSizing";
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

  // NEVER apply a minimum while maximized: on Windows setMinSize restores a
  // maximized window. The deferred constraint lands on the first normal
  // resize instead.
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

  // The Tauri main window starts hidden so WebView2 cannot flash a white
  // frame. Prepare complete usable normal geometry and the stable mode before
  // showing; capture begins only after the resulting native events settle.
  const placementStarting = useRef(false);
  const placementController = useRef<WindowPlacementController | null>(null);
  useEffect(() => {
    if (placementStarting.current) return;
    placementStarting.current = true;
    const appWindow = getCurrentWindow();
    const showFallback = setTimeout(() => {
      void appWindow.show().catch(reportWindowCall("show"));
    }, 3000);
    void (async () => {
      try {
        const state = useAppStore.getState().appData?.state;
        placementController.current = await prepareWindowPlacement({
          window: appWindow,
          saved: placementFromLegacyState(
            state?.windowBounds,
            state?.windowMaximized,
            "maximized",
          ),
          minimum: {
            width: computeMinWindowWidth(splitOpen),
            height: computeMinWindowHeight(),
          },
          monitors: await availableMonitors(),
          persist: async (record) => {
            await useAppStore.getState().patchState({
              windowBounds: record.normalBounds,
              windowMaximized: record.mode === "maximized",
            }, { immediate: true });
          },
          beforeNormalCapture: async () => {
            if (pendingMinSize.current === null) return;
            const size = pendingMinSize.current;
            pendingMinSize.current = null;
            await appWindow.setMinSize(size);
          },
          report: (operation, error) => reportWindowCall(operation)(error),
        });
      } catch (error) {
        reportWindowCall("prepare window placement")(error);
      } finally {
        clearTimeout(showFallback);
        await appWindow.show().catch(reportWindowCall("show"));
        await appWindow.setFocus().catch(reportWindowCall("boot setFocus"));
        await placementController.current?.activate();
      }
    })();
  }, [appData, splitOpen]);

  // Main-window close is also the application quit edge. The Rust menu routes
  // Cmd/Ctrl+Q here, so both durable windows flush before ordinary shutdown
  // quiescence begins.
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
        await Promise.all([
          placementController.current?.flush(),
          flushPreviewWindowPlacement(),
        ]);
      } catch (error) {
        reportStatePatchFailure(error);
      }
      try {
        await invoke("request_app_exit");
      } catch (error) {
        closing = false;
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
      if (isComposingEvent(event) || hasOpenModal()) return;
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
