// The preview: ONE surface in one of two placements. Multi-monitor puts it in
// the `preview` window on screen 2; a single monitor uses a split pane above
// the grid in the main window — a separate window there would cover the grid
// and steal the keyboard focus arrow navigation depends on.
//
// Follow model (FastStone's): `follow` on means the surface tracks the grid
// anchor live. Opening the preview by ANY route turns follow on; the surface
// closing by any route turns it off — one flag, no half-open states. The flag
// persists as app state (`previewFollow`).
//
// The follow path is throttled (leading edge + trailing coalesce) so holding
// an arrow key sends a bounded stream, and the anchor's ItemDetail rides IN
// the payload — the window never re-queries, so the stale-response race
// (wrong filename beside the right image) cannot happen.

import { create } from "zustand";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  PhysicalPosition,
  PhysicalSize,
  availableMonitors,
  getCurrentWindow,
} from "@tauri-apps/api/window";
import { parseSavedBounds, restorableBounds } from "../utils/windowBounds";
import { emit } from "@tauri-apps/api/event";
import { log, toErrorFields, reportWindowCall } from "../repositories";
import { orderMonitors, priorityFromState } from "../utils/screens";
import type { ItemDetail } from "../models/items";

export interface PreviewPayload {
  hash: string | null;
  pathId: number | null;
}

export interface PreviewShowMessage extends PreviewPayload {
  detail: ItemDetail | null;
  seekMs?: number;
  playAfterSeek?: boolean;
}

export type PreviewIntent = Pick<PreviewShowMessage, "seekMs" | "playAfterSeek">;

/** Where the user wants the preview; `null` means never chosen, which is the
 * in-window pane. Purely the user's statement — monitor counting left this
 * path entirely (the developer's call: two windows on halves of one screen,
 * or one window on one screen of three, are the user's business, and any
 * auto-rule makes one of those impossible to ask for). */
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
  /** Opens the surface for the payload and turns follow on. Presentation
   * intent is one named object so adding another medium-specific action does
   * not grow a positional-argument protocol. */
  open: (
    payload: PreviewPayload,
    detail: ItemDetail | null,
    intent?: PreviewIntent,
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

// ---- Window-placement plumbing --------------------------------------------

// Cached existence flag: getByLabel per keystroke is an IPC round trip.
let previewWindowOpen = false;

async function ensurePreviewWindow(state: Record<string, unknown>): Promise<void> {
  const existing = await WebviewWindow.getByLabel("preview");
  if (existing !== null) {
    previewWindowOpen = true;
    return;
  }
  // A PLAIN window that remembers (Phase 33, superseding both earlier
  // z-order designs): its own position, size, and maximized flag persist in
  // state.json — written by the preview window itself — and nothing else.
  // Never topmost: permanent always-on-top floated over OTHER APPS, which is
  // obnoxious; the raise PULSE in frontPreviewWindow does the fronting.
  // Created hidden so the restore is never seen as a jump.
  const window = new WebviewWindow("preview", {
    url: "index.html?view=preview",
    title: "OneCopy Preview",
    width: 1280,
    height: 800,
    visible: false,
  });
  await new Promise<void>((resolve, reject) => {
    void window.once("tauri://created", () => resolve());
    void window.once("tauri://error", (e) => reject(e.payload));
  });
  previewWindowOpen = true;
  // The surface closing by any route (Escape in it, red button) clears the
  // follow flag — otherwise P looks broken afterwards.
  void window.once("tauri://destroyed", () => {
    previewWindowOpen = false;
    const store = usePreviewStore.getState();
    if (store.placement === "window") {
      // The placement PREFERENCE survives — closing the window means "not
      // now", not "never on that screen again".
      usePreviewStore.setState({ follow: false, placement: null, current: null });
    }
  });
  try {
    const monitors = await availableMonitors();
    const saved = restorableBounds(
      parseSavedBounds(state.previewWindowBounds),
      monitors as never,
    );
    if (saved !== null) {
      // The remembered geometry IS the configuration (the half-monitor user
      // sized it once; it comes back exactly there).
      await window.setPosition(new PhysicalPosition(saved.x, saved.y));
      await window.setSize(new PhysicalSize(saved.width, saved.height));
    } else if (monitors.length >= 2) {
      // Nothing remembered: a POSITION nicety only — priority slot 2.
      const ordered = orderMonitors(
        monitors,
        priorityFromState(state),
      );
      await window.setPosition(ordered[1].position);
    }
    // First-ever open defaults to maximized (this is an enlarge-and-view
    // surface); after that the flag is the window's own remembered state.
    if (state.previewWindowMaximized !== false) {
      await window.maximize();
    }
    await window.show().catch(reportWindowCall("preview show"));
    await raisePulse(window);
    // Keep the keyboard where the culling happens.
    await getCurrentWindow().setFocus().catch(reportWindowCall("main setFocus"));
  } catch (error) {
    log.warn("preview window placement failed", toErrorFields(error));
  }
}

export async function restorePreviewAfterComparison(): Promise<void> {
  const { follow, placement } = usePreviewStore.getState();
  if (!follow || placement !== "window") return;
  await frontPreviewWindow();
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
  const existing = await WebviewWindow.getByLabel("preview").catch((error) => {
    reportWindowCall("preview lookup")(error);
    return null;
  });
  if (existing === null) return;
  await existing.show().catch(reportWindowCall("preview show"));
  await raisePulse(existing);
}

// ---- Follow throttle ------------------------------------------------------

const FOLLOW_THROTTLE_MS = 120;
let lastSentAt = 0;
let pending: PreviewShowMessage | null = null;
let trailingTimer: ReturnType<typeof setTimeout> | null = null;

function deliver(message: PreviewShowMessage): void {
  const { placement } = usePreviewStore.getState();
  // `current` is the last-shown message in EITHER placement (the split pane
  // renders it; for the window it is the stale-guard baseline).
  usePreviewStore.setState({ current: message });
  if (placement === "window" && previewWindowOpen) {
    void emit("preview://show", message);
  }
}

function throttledDeliver(message: PreviewShowMessage): void {
  const now = Date.now();
  if (now - lastSentAt >= FOLLOW_THROTTLE_MS) {
    lastSentAt = now;
    deliver(message);
    return;
  }
  pending = message;
  if (trailingTimer === null) {
    trailingTimer = setTimeout(() => {
      trailingTimer = null;
      if (pending !== null) {
        lastSentAt = Date.now();
        const message = pending;
        pending = null;
        deliver(message);
      }
    }, FOLLOW_THROTTLE_MS);
  }
}

// ---- The store ------------------------------------------------------------

export const usePreviewStore = create<PreviewState>((set, get) => ({
  follow: false,
  placement: null,
  placementPreference: null,
  current: null,

  open: async (payload, detail, intent = {}, windowState = {}) => {
    try {
      const placement = resolvePlacement(get().placementPreference);
      // State FIRST: the side pane renders `current` the moment this lands,
      // which is what makes the image appear immediately on activation.
      const message = { ...payload, detail, ...intent };
      set({ follow: true, placement, current: message });
      if (placement === "window") {
        await ensurePreviewWindow(windowState);
        await frontPreviewWindow();
        // A freshly created webview misses this emit (still booting) — its
        // ready announcement fetches the current state instead; an already
        // -open window hears it directly.
        void emit("preview://show", message);
      }
    } catch (error) {
      log.error("preview open failed", toErrorFields(error));
    }
  },

  close: () => {
    const { placement } = get();
    set({ follow: false, placement: null, current: null });
    if (placement === "window") {
      void WebviewWindow.getByLabel("preview").then((w) => w?.close());
    }
  },

  setPlacementPreference: async (preference, windowState = {}) => {
    const { follow, placement, current } = get();
    set({ placementPreference: preference });
    if (!follow) return;
    const next = resolvePlacement(preference);
    if (next === placement) return;
    // The new placement is published BEFORE the old window is torn down.
    // The order is load-bearing: the preview window's tauri://destroyed
    // handler treats "destroyed while placement is still 'window'" as the
    // user closing the window and turns follow OFF — so closing first made
    // every window→inline switch read as a manual close and disabled the
    // preview the user was in the middle of moving. With placement already
    // "split", that handler stands down and follow survives the switch.
    set({ placement: next });
    // Tearing the old surface down still happens: leaving the preview window
    // open behind a split pane would show the same photo twice and keep a
    // window the user just asked to be rid of.
    if (placement === "window") {
      await WebviewWindow.getByLabel("preview").then((w) => w?.close());
    }
    if (next === "window" && current !== null) {
      await ensurePreviewWindow(windowState);
      await frontPreviewWindow();
      void emit("preview://show", current);
    }
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
    deliver({ hash: null, pathId: null, detail: null });
  },

  anchorChanged: (payload, detail) => {
    const { follow, placement } = get();
    if (!follow) return;
    if (placement === null) {
      // A restored follow flag is opened by the item workflow, which can
      // supply the app-owned window geometry without a peer-store import.
      return;
    }
    throttledDeliver({ ...payload, detail });
  },

  detailLoaded: (payload, detail) => {
    const { follow, placement, current } = get();
    if (!follow || placement === null) return;
    // Complete the earlier hash-only message — SAME item only; a slow detail
    // for a superseded anchor must never paint the wrong name (the stale
    // race the old double-fetch had).
    if (current !== null && current.hash === payload.hash && current.pathId === payload.pathId) {
      deliver({ ...payload, detail });
    }
  },
}));
