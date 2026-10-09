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
import { orderMonitors, priorityFromConfig } from "../utils/screens";
import type { ItemDetail } from "../models/items";
import { message, type Message } from "../i18n/translate";
import { documentTranslator } from "../i18n/I18nContext";
import { recordActionFailure } from "./notifications-store";
import { recordActivity } from "../repositories/activity";
import { waitForWindowCreated } from "../utils/windowCreation";

export interface PreviewPayload {
  hash: string | null;
  pathId: number | null;
  /** Main's current selection size, so a multi-selection anchor can show
   * `N selected` beside it. Absent when the caller does not project Main's
   * live selection (for example the scene-strip's same-item reopen). */
  selectedCount?: number;
}

export interface PreviewShowMessage extends PreviewPayload {
  detail: ItemDetail | null;
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
  /** True once an `open` attempt has failed and stays failed until the user
   * acts (placement change, close, or a fresh restore). While true, the
   * anchor-driven auto-reopen in the item workflow stands down instead of
   * retrying — and failing — on every subsequent anchor change. */
  openFailed: boolean;
  /** Result owned by the Preview command surface, never the global host. A
   * descriptor, not words: the preview window renders it in its own language,
   * and follows a language change while it stays on screen. */
  error: Message | null;
  clearError: () => void;
  /** Opens the surface for the payload and turns follow on. */
  open: (
    payload: PreviewPayload,
    detail: ItemDetail | null,
    config?: Record<string, unknown>,
  ) => Promise<void>;
  /** Closes the surface (either placement) and turns follow off. */
  close: () => void;
  /** Moves the open surface to the other placement, and remembers the choice. */
  setPlacementPreference: (
    preference: PlacementPreference,
    config?: Record<string, unknown>,
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

async function placePreviewWindow(config: Record<string, unknown>): Promise<void> {
  const main = getCurrentWindow();
  const [monitors, mainBounds] = await Promise.all([
    availableMonitors(),
    outerBounds(main),
  ]);
  const allocation = allocatePreviewPlacement(
    orderMonitors(monitors, priorityFromConfig(config)),
    mainBounds,
  );
  if (allocation === null) return;
  await invoke("place_preview_window", {
    normal: allocation.normalBounds,
    maximized: allocation.maximized,
  });
}

async function ensurePreviewWindow(config: Record<string, unknown>): Promise<void> {
  const existing = await WebviewWindow.getByLabel("preview");
  if (existing !== null) {
    previewWindowOpen = true;
    return;
  }
  // Prepare while hidden and unfocused. Raising must not activate Main over
  // an overlapping Preview; the temporary raise pulse preserves command focus.
  const window = new WebviewWindow("preview", {
    url: "index.html?view=preview",
    // The definitive title follows the language boot read (window-appearance.ts);
    // this creation-time value is only what shows
    // for the brief instant before that first paint.
    title: documentTranslator().t("window.titlePreview"),
    width: 1280,
    height: 800,
    visible: false,
    focus: false,
  });
  try {
    await waitForWindowCreated(window, "Preview");
    await placePreviewWindow(config);
    // The surface closing by any route (Escape in it, red button) clears the
    // follow flag — otherwise P looks broken afterwards.
    await window.once("tauri://destroyed", () => {
      previewWindowOpen = false;
      const store = usePreviewStore.getState();
      if (store.placement === "window") {
        // The placement PREFERENCE survives — closing the window means "not
        // now", not "never on that screen again".
        surfaceRequest += 1;
        cancelPublication();
        usePreviewStore.setState({ follow: false, placement: null, current: null });
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
      message("preview.restoreFailed"),
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

/** Rust samples the window's placement and then destroys it, since a
 * destroyed window sends no close request to sample at. */
async function closePreviewWindow(): Promise<void> {
  await invoke("close_preview_window");
}

function publishPreviewFailure(
  kind: string,
  failure: Message,
  error: unknown,
): void {
  log.error("preview action failed", { kind, ...toErrorFields(error) });
  usePreviewStore.setState({ error: failure });
  recordActionFailure(kind, failure, error);
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
          message("preview.updateFailed"),
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
  openFailed: false,
  error: null,
  clearError: () => set({ error: null }),

  open: async (payload, detail, config = {}) => {
    const request = ++surfaceRequest;
    cancelPublication();
    try {
      const placement = resolvePlacement(get().placementPreference);
      // State FIRST: the side pane renders `current` the moment this lands,
      // which is what makes the image appear immediately on activation.
      const message = { ...payload, detail };
      set({ follow: true, placement, current: message, error: null, openFailed: false });
      if (placement === "window") {
        await enqueueSurface(async () => {
          if (request !== surfaceRequest) {
            recordStaleSurface(request);
            return;
          }
          await ensurePreviewWindow(config);
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
      // Stay armed but stop auto-retrying: the item workflow's anchor-driven
      // reopen only fires while `placement` is null AND `openFailed` is
      // false, so a real failure no longer reopens (and re-fails) on every
      // subsequent arrow key. The user's own placement choice (the "show in
      // this window" offer) clears this flag and tries again explicitly.
      set({ placement: null, openFailed: true });
      publishPreviewFailure(
        "preview-open-failed",
        message("preview.openFailed"),
        error,
      );
    }
  },

  // Closing Preview leaves Main's selection and anchor as they are.
  close: () => {
    surfaceRequest += 1;
    cancelPublication();
    const { placement } = get();
    set({ follow: false, placement: null, current: null, openFailed: false });
    if (placement === "window") {
      void enqueueSurface(async () => {
        await closePreviewWindow();
      });
    }
  },

  setPlacementPreference: async (preference, config = {}) => {
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
      set({ placement: next, error: null, openFailed: false });
      try {
        if (placement === "window") {
          await closePreviewWindow();
        }
        if (request !== surfaceRequest) {
          recordStaleSurface(request);
          return;
        }
        if (next === "window" && current !== null) {
          await ensurePreviewWindow(config);
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
          message("preview.placementChangeFailed"),
          error,
        );
      }
    });
  },

  restoreFollow: (on, preference) => {
    set({ follow: on, placementPreference: preference, openFailed: false });
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
