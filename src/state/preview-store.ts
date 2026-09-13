// Preview is one Main follower, in a user-chosen pane or separate window.
// Follow model (FastStone's): `follow` on means the surface tracks the grid
// anchor live. Opening the preview by ANY route turns follow on; the surface
// closing by any route turns it off — one flag, no half-open states. The flag
// persists as app state (`previewFollow`).
//
// `current` is the latest anchor and its matching detail, never a delayed
// selection. Only cross-window publication is throttled; each publication
// reads this one package so loading detail cannot race a queued identity.

import { create } from "zustand";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  availableMonitors,
  getCurrentWindow,
} from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields, reportWindowCall } from "../repositories";
import {
  allocatePreviewPlacement,
  type PreviewBounds,
} from "../models/previewPlacement";
import { orderMonitors, priorityFromState } from "../utils/screens";
import type { ItemDetail } from "../models/items";
import { recordActionFailure } from "./notifications-store";
import { recordActivity } from "../repositories/activity";
import { waitForWindowCreated } from "../utils/windowCreation";

export interface PreviewPayload {
  hash: string | null;
  pathId: number | null;
}

export interface PreviewShowMessage extends PreviewPayload {
  detail: ItemDetail | null;
}

export interface PreviewPresentation {
  fullscreen: boolean;
  error: string | null;
}

/** Pane versus separate window remains an explicit user choice. */
export type PlacementPreference = "split" | "window" | null;

export function resolvePlacement(preference: PlacementPreference): "window" | "split" {
  return preference === "window" ? "window" : "split";
}

interface PreviewState {
  /** The surface follows the grid anchor while true (persisted). */
  follow: boolean;
  /** Which placement the open surface uses; null while closed. */
  placement: "window" | "split" | null;
  /** The user's stated placement, independent of whether it is open. */
  placementPreference: PlacementPreference;
  /** What the side pane renders (the window renders from events). */
  current: PreviewShowMessage | null;
  /** Requested presentation of the same live separate-window follower. */
  fullscreen: boolean;
  setFullscreen: (enabled: boolean) => Promise<void>;
  /** Result owned by the Preview command surface, never the global host. */
  error: string | null;
  clearError: () => void;
  /** Opens the surface for the payload and turns follow on. */
  open: (
    payload: PreviewPayload,
    detail: ItemDetail | null,
    windowState?: Record<string, unknown>,
  ) => Promise<void>;
  /** Closes the surface (either placement) and turns follow off. */
  close: () => void;
  /** Moves the open surface to the other placement, and remembers the choice. */
  setPlacementPreference: (
    preference: PlacementPreference,
    windowState?: Record<string, unknown>,
  ) => Promise<void>;
  /** Restores the persisted follow flag without opening anything yet. */
  restoreFollow: (on: boolean, preference: PlacementPreference) => void;
  /** The selection emptied: clear the surface, keep follow armed. */
  anchorCleared: () => void;
  /** The anchor moved: feed the surface if follow is on. */
  anchorChanged: (payload: PreviewPayload, detail: ItemDetail | null) => void;
  /** The anchor's detail finished loading: complete the earlier message. */
  detailLoaded: (payload: PreviewPayload, detail: ItemDetail) => void;
}

// ---- Separate-window lifecycle --------------------------------------------

// Cached existence flag: getByLabel per keystroke is an IPC round trip.
let previewWindowOpen = false;
let surfaceRequest = 0;
let surfaceTail: Promise<void> = Promise.resolve();
let previewFullscreenApplied = false;

function enqueueSurface(task: () => Promise<void>): Promise<void> {
  const operation = surfaceTail.then(task, task);
  surfaceTail = operation.catch(() => undefined);
  return operation;
}

function recordStaleSurface(generation: number): void {
  recordActivity({
    kind: "stale",
    owner: "preview",
    generation,
    current: "stale",
    reason: "staleResponse",
  });
}

async function outerBounds(window: {
  outerPosition: () => Promise<{ x: number; y: number }>;
  outerSize: () => Promise<{ width: number; height: number }>;
}): Promise<PreviewBounds> {
  const [position, size] = await Promise.all([
    window.outerPosition(),
    window.outerSize(),
  ]);
  return {
    x: position.x,
    y: position.y,
    width: size.width,
    height: size.height,
  };
}

async function placePreviewWindow(state: Record<string, unknown>): Promise<void> {
  const main = getCurrentWindow();
  const [monitors, mainBounds] = await Promise.all([
    availableMonitors(),
    outerBounds(main),
  ]);
  const allocation = allocatePreviewPlacement(
    orderMonitors(monitors, priorityFromState(state)),
    mainBounds,
  );
  if (allocation === null) return;
  await invoke("place_preview_window", {
    normal: allocation.normalBounds,
    maximized: allocation.maximized,
  });
}

async function ensurePreviewWindow(state: Record<string, unknown>): Promise<void> {
  const existing = await WebviewWindow.getByLabel("preview");
  if (existing !== null) {
    previewWindowOpen = true;
    return;
  }
  // Prepare while hidden and unfocused. Raising must not activate Main over
  // an overlapping Preview; the temporary raise pulse preserves command focus.
  const window = new WebviewWindow("preview", {
    url: "index.html?view=preview",
    title: "OneCopy Preview",
    width: 1280,
    height: 800,
    visible: false,
    focus: false,
  });
  try {
    await waitForWindowCreated(window, "Preview");
    await placePreviewWindow(state);
    // The surface closing by any route (Escape in it, red button) clears the
    // follow flag — otherwise P looks broken afterwards.
    await window.once("tauri://destroyed", () => {
      previewWindowOpen = false;
      previewFullscreenApplied = false;
      const store = usePreviewStore.getState();
      if (store.placement === "window") {
        // The placement PREFERENCE survives — closing the window means "not
        // now", not "never on that screen again".
        surfaceRequest += 1;
        cancelPublication();
        usePreviewStore.setState({ follow: false, placement: null, current: null, fullscreen: false });
      }
    });
  } catch (error) {
    await window.close().catch(reportWindowCall("preview cleanup after listener failure"));
    throw error;
  }
  try {
    let closing = false;
    await window.onCloseRequested(async (event) => {
      event.preventDefault();
      if (closing) return;
      closing = true;
      try {
        await closePreviewWindow();
      } catch (error) {
        closing = false;
        reportWindowCall("preview close")(error);
      }
    });
  } catch (error) {
    await window.destroy().catch(reportWindowCall("preview cleanup after close listener failure"));
    throw error;
  }
  previewWindowOpen = true;
}

export async function restorePreviewAfterComparison(): Promise<void> {
  const { follow, placement } = usePreviewStore.getState();
  if (!follow || placement !== "window") return;
  try {
    await frontPreviewWindow();
  } catch (error) {
    publishPreviewFailure(
      "preview-restore-failed",
      "Couldn’t restore the Preview window.",
      error,
    );
  }
}

/** Raise WITHOUT stealing: a topmost pulse leaves the window above the main
 * window (Windows' documented TOPMOST→NOTOPMOST front placement; macOS keeps
 * the front ordering after the level drop) while the keyboard never moves —
 * and unlike a standing always-on-top, it floats over no other app. */
async function raisePulse(window: WebviewWindow): Promise<void> {
  await window.setAlwaysOnTop(true).catch(reportWindowCall("preview setAlwaysOnTop"));
  await window.setAlwaysOnTop(false).catch(reportWindowCall("preview setAlwaysOnTop"));
}

/** Reveals an already-existing preview window: show, then the raise pulse.
 * No focus call in either direction — the old focus-the-preview-then-
 * refocus-main dance raised MAIN over an overlapping preview and made Space
 * look dead. */
async function frontPreviewWindow(): Promise<void> {
  const existing = await WebviewWindow.getByLabel("preview");
  if (existing === null) throw new Error("The Preview window is unavailable.");
  await existing.show();
  await raisePulse(existing);
}

async function closePreviewWindow(): Promise<void> {
  const existing = await WebviewWindow.getByLabel("preview");
  if (existing === null) return;
  await invoke("capture_preview_window_placement");
  await invoke("set_window_fullscreen", { label: "preview", enable: false });
  previewFullscreenApplied = false;
  await existing.destroy();
}

function publishPreviewFailure(
  kind: string,
  message: string,
  error: unknown,
): void {
  log.error("preview action failed", { kind, ...toErrorFields(error) });
  usePreviewStore.setState({ error: message });
  recordActionFailure(kind, message, error);
}

// ---- Cross-window publication --------------------------------------------

const FOLLOW_THROTTLE_MS = 120;
let lastSentAt: number | null = null;
let trailingTimer: ReturnType<typeof setTimeout> | null = null;

function cancelPublication(): void {
  if (trailingTimer !== null) clearTimeout(trailingTimer);
  trailingTimer = null;
  lastSentAt = null;
}

function publishCurrent(): void {
  const { follow, placement, current } = usePreviewStore.getState();
  if (follow && placement === "window" && previewWindowOpen && current !== null) {
    const request = surfaceRequest;
    lastSentAt = Date.now();
    void emit("preview://show", current)
      .then(() => {
        if (request === surfaceRequest) usePreviewStore.setState({ error: null });
      })
      .catch((error) =>
        request === surfaceRequest && publishPreviewFailure(
          "preview-update-failed",
          "Couldn’t update the Preview window.",
          error,
        ),
      );
  }
}

function schedulePublication(): void {
  const { follow, placement } = usePreviewStore.getState();
  if (!follow || placement !== "window" || !previewWindowOpen) return;
  const now = Date.now();
  if (lastSentAt === null || now - lastSentAt >= FOLLOW_THROTTLE_MS) {
    cancelPublication();
    publishCurrent();
    return;
  }
  if (trailingTimer === null) {
    trailingTimer = setTimeout(() => {
      trailingTimer = null;
      publishCurrent();
    }, FOLLOW_THROTTLE_MS - (now - lastSentAt));
  }
}

// ---- The store ------------------------------------------------------------

export const usePreviewStore = create<PreviewState>((set, get) => ({
  follow: false,
  placement: null,
  placementPreference: null,
  current: null,
  fullscreen: false,
  error: null,
  clearError: () => set({ error: null }),

  setFullscreen: async (enabled) => {
    if (!get().follow || get().placement !== "window") return;
    const request = surfaceRequest;
    set({ fullscreen: enabled, error: null });
    try {
      await enqueueSurface(async () => {
        if (request !== surfaceRequest) return;
        const window = await WebviewWindow.getByLabel("preview");
        if (window === null) throw new Error("The Preview window is unavailable.");
        if (enabled) await invoke("capture_preview_window_placement");
        await invoke("set_window_fullscreen", { label: "preview", enable: enabled });
        previewFullscreenApplied = enabled;
        if (request === surfaceRequest) await window.setFocus();
      });
    } catch (error) {
      if (request !== surfaceRequest) return;
      set({ fullscreen: previewFullscreenApplied });
      publishPreviewFailure("preview-fullscreen-failed", "Couldn’t change Preview full screen.", error);
    }
  },

  open: async (payload, detail, windowState = {}) => {
    const request = ++surfaceRequest;
    cancelPublication();
    try {
      const placement = resolvePlacement(get().placementPreference);
      // State FIRST: the side pane renders `current` the moment this lands,
      // which is what makes the image appear immediately on activation.
      const message = { ...payload, detail };
      set({ follow: true, placement, current: message, error: null });
      if (placement === "window") {
        await enqueueSurface(async () => {
          if (request !== surfaceRequest) {
            recordStaleSurface(request);
            return;
          }
          await ensurePreviewWindow(windowState);
          if (request !== surfaceRequest) {
            recordStaleSurface(request);
            return;
          }
          await frontPreviewWindow();
          if (request !== surfaceRequest) {
            recordStaleSurface(request);
            return;
          }
          // A freshly created webview misses this emit (still booting) — its
          // ready announcement fetches the current state instead; an already
          // -open window hears it directly.
          cancelPublication();
          publishCurrent();
        });
      }
    } catch (error) {
      if (request !== surfaceRequest) {
        recordStaleSurface(request);
        return;
      }
      set({ placement: null });
      publishPreviewFailure(
        "preview-open-failed",
        "Couldn’t open the Preview window.",
        error,
      );
    }
  },

  close: () => {
    surfaceRequest += 1;
    cancelPublication();
    const { placement } = get();
    set({ follow: false, placement: null, current: null, fullscreen: false });
    if (placement === "window") {
      void enqueueSurface(async () => {
        await closePreviewWindow();
      });
    }
  },

  setPlacementPreference: async (preference, windowState = {}) => {
    const request = ++surfaceRequest;
    const { follow, placementPreference } = get();
    set({ placementPreference: preference });
    if (!follow) return;
    const next = resolvePlacement(preference);
    await enqueueSurface(async () => {
      if (request !== surfaceRequest) {
        recordStaleSurface(request);
        return;
      }
      const { placement, current } = get();
      if (next === placement) return;
      cancelPublication();
      // The new placement is published BEFORE the old window is torn down.
      // The order is load-bearing: the preview window's destroyed handler
      // treats destruction while placement is still `window` as a manual
      // close and turns follow off.
      set({ placement: next, fullscreen: false, error: null });
      try {
        if (placement === "window") {
          await closePreviewWindow();
        }
        if (request !== surfaceRequest) {
          recordStaleSurface(request);
          return;
        }
        if (next === "window" && current !== null) {
          await ensurePreviewWindow(windowState);
          if (request !== surfaceRequest) {
            recordStaleSurface(request);
            return;
          }
          await frontPreviewWindow();
          if (request !== surfaceRequest) {
            recordStaleSurface(request);
            return;
          }
          cancelPublication();
          publishCurrent();
        }
      } catch (error) {
        if (request !== surfaceRequest) {
          recordStaleSurface(request);
          return;
        }
        set({ placement: placement ?? null, placementPreference });
        publishPreviewFailure(
          "preview-placement-failed",
          "Couldn’t change the Preview placement.",
          error,
        );
      }
    });
  },

  restoreFollow: (on, preference) => {
    set({ follow: on, placementPreference: preference });
  },

  // The selection emptied — deselected to nothing, or its last item trashed.
  // The surface goes blank rather than holding the previous photo (which for
  // a trashed file was a small lie); follow stays armed for the next anchor.
  anchorCleared: () => {
    const { follow, placement } = get();
    if (!follow || placement === null) return;
    cancelPublication();
    set({ current: { hash: null, pathId: null, detail: null } });
    publishCurrent();
  },

  anchorChanged: (payload, detail) => {
    const { follow, placement } = get();
    if (!follow) return;
    if (placement === null) {
      // A restored follow flag is opened by the item workflow once an anchor
      // is available.
      return;
    }
    set({ current: { ...payload, detail } });
    schedulePublication();
  },

  detailLoaded: (payload, detail) => {
    const { follow, placement, current } = get();
    if (!follow || placement === null) return;
    // Complete the earlier hash-only message — SAME item only; a slow detail
    // for a superseded anchor must never paint the wrong name (the stale
    // race the old double-fetch had).
    if (current !== null && current.hash === payload.hash && current.pathId === payload.pathId) {
      set({ current: { ...payload, detail } });
      schedulePublication();
    }
  },
}));
