// Native behavior of the main window. App renders the shell; this hook owns
// the physical window/webview lifetime and the persisted geometry/zoom state.

import { useCallback, useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import {
  availableMonitors,
  currentMonitor,
  getCurrentWindow,
  LogicalSize,
  PhysicalPosition,
  PhysicalSize,
} from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { LoadedAppData } from "../repositories";
import { reportWindowCall } from "../repositories";
import { useAppStore } from "../state/app-store";
import { installActivityPings } from "../state/derived-work-store";
import { usePreviewStore } from "../state/preview-store";
import { hasOpenModal } from "../utils/modalStack";
import { isComposingEvent } from "./useComposing";
import { parseSavedBounds, restorableBounds, shrinkToFit } from "../utils/windowBounds";
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
  appData: LoadedAppData | null;
  loadError: string | null;
  splitOpen: boolean;
}

export function useMainWindowLifecycle({
  appData,
  loadError,
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
  // frame. Restore reachable normal geometry, restore maximized separately,
  // and show even if monitor discovery fails.
  const bootShown = useRef(false);
  useEffect(() => {
    if (bootShown.current || (appData === null && loadError === null)) return;
    bootShown.current = true;
    const appWindow = getCurrentWindow();
    const showFallback = setTimeout(() => {
      void appWindow.show().catch(reportWindowCall("show"));
    }, 3000);
    void (async () => {
      try {
        const state = useAppStore.getState().appData?.state;
        const saved = restorableBounds(
          parseSavedBounds(state?.windowBounds),
          await availableMonitors(),
        );
        if (saved !== null) {
          await appWindow.setPosition(new PhysicalPosition(saved.x, saved.y));
          await appWindow.setSize(new PhysicalSize(saved.width, saved.height));
        } else {
          const monitor = await currentMonitor();
          const inner = await appWindow.innerSize();
          const fitted = monitor !== null ? shrinkToFit(inner, monitor.workArea.size) : null;
          if (fitted !== null) {
            await appWindow.setSize(new PhysicalSize(fitted.width, fitted.height));
          }
        }
        if (state?.windowMaximized === true) await appWindow.maximize();
      } catch (error) {
        reportWindowCall("restore bounds")(error);
      } finally {
        clearTimeout(showFallback);
        await appWindow.show().catch(reportWindowCall("show"));
        await appWindow.setFocus().catch(reportWindowCall("boot setFocus"));
      }
    })();
  }, [appData, loadError]);

  // Persist only settled normal geometry. Maximized is a flag, never the
  // maximized rectangle, so un-maximizing retains a real landing place.
  useEffect(() => {
    const appWindow = getCurrentWindow();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const save = () => {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        void (async () => {
          try {
            if (await appWindow.isMaximized()) {
              await useAppStore.getState().patchState({ windowMaximized: true });
              return;
            }
            if (pendingMinSize.current !== null) {
              const size = pendingMinSize.current;
              pendingMinSize.current = null;
              await appWindow.setMinSize(size).catch(reportWindowCall("setMinSize"));
            }
            const position = await appWindow.outerPosition();
            const size = await appWindow.innerSize();
            await useAppStore.getState().patchState({
              windowMaximized: false,
              windowBounds: {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
              },
            });
          } catch (error) {
            reportWindowCall("save bounds")(error);
          }
        })();
      }, 500);
    };
    const unlistens: Array<() => void> = [];
    void appWindow.onMoved(save).then((fn) => unlistens.push(fn));
    void appWindow.onResized(save).then((fn) => unlistens.push(fn));
    return () => {
      if (timer !== null) clearTimeout(timer);
      for (const fn of unlistens) fn();
    };
  }, []);

  const zoomRef = useRef(ZOOM_DEFAULT);
  const [zoomLevel, setZoomLevel] = useState(ZOOM_DEFAULT);
  const applyZoom = useCallback((next: number) => {
    zoomRef.current = next;
    setZoomLevel(next);
    void getCurrentWebview().setZoom(next).catch(reportWindowCall("setZoom"));
    void useAppStore.getState().patchState({ zoomLevel: next });
  }, []);

  useEffect(() => {
    if (appData === null) return;
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
