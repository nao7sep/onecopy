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
import { availableMonitors, getCurrentWindow } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";
import { orderMonitors, priorityFromState } from "../utils/screens";
import type { ItemDetail } from "./items-store";

export interface PreviewPayload {
  hash: string | null;
  pathId: number | null;
}

export interface PreviewShowMessage extends PreviewPayload {
  detail: ItemDetail | null;
}

interface PreviewState {
  /** The surface follows the grid anchor while true (persisted). */
  follow: boolean;
  /** Which placement the open surface uses; null while closed. */
  placement: "window" | "split" | null;
  /** What the split pane renders (the window renders from events). */
  current: PreviewShowMessage | null;
  /** Split ratio: the preview's share of the main content column (persisted). */
  splitRatio: number;
  setSplitRatio: (ratio: number) => void;
  /** Opens the surface for the payload and turns follow on. */
  open: (payload: PreviewPayload, detail: ItemDetail | null) => Promise<void>;
  /** Closes the surface (either placement) and turns follow off. */
  close: () => void;
  /** Restores the persisted follow flag without opening anything yet. */
  restoreFollow: (on: boolean, ratio: number | null) => void;
  /** The anchor moved: feed the surface if follow is on. */
  anchorChanged: (payload: PreviewPayload, detail: ItemDetail | null) => void;
  /** The anchor's detail finished loading: complete the earlier message. */
  detailLoaded: (payload: PreviewPayload, detail: ItemDetail) => void;
}

// ---- Window-placement plumbing --------------------------------------------

// Cached existence flag: getByLabel per keystroke is an IPC round trip.
let previewWindowOpen = false;

async function ensurePreviewWindow(): Promise<void> {
  const existing = await WebviewWindow.getByLabel("preview");
  if (existing !== null) {
    previewWindowOpen = true;
    return;
  }
  const window = new WebviewWindow("preview", {
    url: "index.html?view=preview",
    title: "OneCopy Preview",
    width: 1280,
    height: 800,
    // Never steal the keyboard from the grid being culled.
    focus: false,
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
      usePreviewStore.setState({ placement: null });
      store.restoreFollow(false, null);
      persistFollow(false);
    }
  });
  try {
    const monitors = await availableMonitors();
    if (monitors.length >= 2) {
      // Screen priority: slot 2 of the ordered list is the preview screen.
      const { useAppStore } = await import("./app-store");
      const ordered = orderMonitors(
        monitors,
        priorityFromState(useAppStore.getState().appData?.state ?? null),
      );
      await window.setPosition(ordered[1].position);
    }
    // Keep the keyboard where the culling happens.
    await getCurrentWindow().setFocus().catch(() => {});
  } catch (error) {
    log.warn("preview window placement failed", toErrorFields(error));
  }
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

function persistFollow(on: boolean): void {
  void import("./app-store").then(({ useAppStore }) =>
    useAppStore.getState().patchState({ previewFollow: on }),
  );
}

// ---- The store ------------------------------------------------------------

export const usePreviewStore = create<PreviewState>((set, get) => ({
  follow: false,
  placement: null,
  current: null,
  splitRatio: 0.5,

  setSplitRatio: (ratio) => {
    const clamped = Math.min(0.85, Math.max(0.15, ratio));
    set({ splitRatio: clamped });
  },

  open: async (payload, detail) => {
    try {
      const monitors = await availableMonitors().catch(() => []);
      const placement = monitors.length >= 2 ? "window" : "split";
      set({ follow: true, placement, current: { ...payload, detail } });
      persistFollow(true);
      if (placement === "window") {
        await ensurePreviewWindow();
        void emit("preview://show", { ...payload, detail });
      }
    } catch (error) {
      log.error("preview open failed", toErrorFields(error));
    }
  },

  close: () => {
    const { placement } = get();
    set({ follow: false, placement: null, current: null });
    persistFollow(false);
    if (placement === "window") {
      void WebviewWindow.getByLabel("preview").then((w) => w?.close());
    }
  },

  restoreFollow: (on, ratio) => {
    set({
      follow: on,
      ...(ratio !== null && Number.isFinite(ratio)
        ? { splitRatio: Math.min(0.85, Math.max(0.15, ratio)) }
        : {}),
    });
  },

  anchorChanged: (payload, detail) => {
    const { follow, placement } = get();
    if (!follow) return;
    if (placement === null) {
      // Follow restored from state but the surface not opened yet this
      // session: open it on the first anchor.
      void get().open(payload, detail);
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

/** Back-compat entry for the Enter key path. */
export async function showPreview(payload: PreviewPayload): Promise<void> {
  const { useItemsStore } = await import("./items-store");
  await usePreviewStore.getState().open(payload, useItemsStore.getState().detail);
}
