// The transient viewer is one application-owned session with two renderings:
// an overlay in Main and a reusable borderless fullscreen window. This edge
// coordinates those surfaces; the pure sequence rules live in
// models/viewerSession and neither React surface owns library state.

import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { createViewerSession, type ViewerMove, type ViewerPresentation } from "../models/viewerSession";
import type { ItemDetail, SectionItem } from "../models/items";
import { isAudioFile, itemKey, sectionProjection } from "../models/items";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { useQuickViewStore } from "../state/quick-view-store";
import { reportActionFailure } from "../state/notifications-store";
import { deleteItems } from "./items";
import { toggleMainPlayback } from "./playback";
import {
  enterViewerFullscreen,
  exitViewerFullscreen,
  type ViewerMonitor,
} from "./viewer-window";
export type { ViewerMonitor } from "./viewer-window";

export interface ViewerBroadcast {
  item: SectionItem | null;
  detail: ItemDetail | null;
  index: number;
  length: number;
  pendingDelete: "trash" | "permanent" | null;
  sectionKind: "image" | "video" | "other" | null;
}

interface ViewerKeyMessage {
  key: string;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}

let installed = false;
let itemReconcileQueued = false;
let fullscreenRequest = 0;

function currentItem(): SectionItem | null {
  const key = useQuickViewStore.getState().currentKey();
  const state = useItemsStore.getState();
  return key === null
    ? null
    : (sectionProjection(state.items, state.currentSort()).itemByKey.get(key) ?? null);
}

export function viewerBroadcast(): ViewerBroadcast {
  const session = useQuickViewStore.getState().session;
  const item = currentItem();
  const items = useItemsStore.getState();
  return {
    item,
    detail:
      item !== null && items.selectedItem === itemKey(item) ? items.detail : null,
    index: session?.index ?? 0,
    length: session?.members.length ?? 0,
    pendingDelete: useQuickViewStore.getState().pendingDelete,
    sectionKind: items.selected?.kind ?? null,
  };
}

function broadcastViewer(): void {
  if (useQuickViewStore.getState().session?.presentation !== "fullscreen") return;
  void emit("viewer://state", viewerBroadcast()).catch(
    reportWindowCall("viewer state broadcast"),
  );
}

function clearFullscreenSurface(): void {
  void emit("viewer://state", {
    item: null,
    detail: null,
    index: 0,
    length: 0,
    pendingDelete: null,
    sectionKind: null,
  } satisfies ViewerBroadcast).catch(reportWindowCall("viewer clear broadcast"));
}

function syncMainAnchor(): void {
  const viewer = useQuickViewStore.getState();
  const key = viewer.currentKey();
  if (key === null) return;
  if (viewer.session?.scope === "selection") {
    useItemsStore.getState().setAnchor(key);
  } else {
    useItemsStore.getState().selectItem(key);
  }
}

function focusMainAnchor(): void {
  if (typeof document !== "undefined") {
    document.getElementById("main-item-area")?.focus();
  }
}

async function restoreMainFocus(): Promise<void> {
  await getCurrentWindow().setFocus().catch(reportWindowCall("main setFocus"));
  focusMainAnchor();
}

function beginFullscreen(
  fallback: "quick" | "main",
  preferredMonitor?: ViewerMonitor,
): void {
  const request = ++fullscreenRequest;
  void enterViewerFullscreen(preferredMonitor)
    .then(broadcastViewer)
    .catch((error) => recoverFullscreenFailure(error, fallback, request));
}

function recoverFullscreenFailure(
  error: unknown,
  fallback: "quick" | "main",
  request: number,
): void {
  log.error("fullscreen viewer failed", toErrorFields(error));
  if (request !== fullscreenRequest) return;
  const session = useQuickViewStore.getState().session;
  if (session?.presentation === "fullscreen") {
    if (fallback === "quick") {
      useQuickViewStore.getState().setPresentation("quick");
    } else {
      useQuickViewStore.getState().close();
    }
    useItemsStore.setState({ message: "Couldn’t open full screen." });
    reportActionFailure("fullscreen-open-failed", "Couldn’t open full screen.", error);
  }
  clearFullscreenSurface();
  void exitViewerFullscreen().then(restoreMainFocus);
}

/** Installs the cross-window handshake and disappearance reconciliation once. */
export async function installViewerWorkflow(): Promise<void> {
  if (installed) return;
  installed = true;
  await Promise.all([
    listen("viewer://ready", broadcastViewer),
    listen<ViewerKeyMessage>("viewer://key", (event) => {
      void handleViewerKey(event.payload);
    }),
    listen("viewer://confirm-delete", () => {
      void confirmViewerDelete();
    }),
    listen("viewer://cancel-delete", () => {
      useQuickViewStore.getState().cancelDelete();
    }),
    listen<ViewerMonitor | null>("preview://fullscreen", (event) => {
      openViewerFromMain("fullscreen", event.payload ?? undefined);
    }),
  ]);
  useQuickViewStore.subscribe(broadcastViewer);
  useItemsStore.subscribe((state, previous) => {
    if (state.items === previous.items || itemReconcileQueued) return;
    itemReconcileQueued = true;
    queueMicrotask(() => {
      itemReconcileQueued = false;
      const before = useQuickViewStore.getState().currentKey();
      const state = useItemsStore.getState();
      useQuickViewStore.getState().reconcile(
        sectionProjection(state.items, state.currentSort()).orderedItems.map((item) => ({
          key: itemKey(item),
          pathId: item.pathId,
        })),
      );
      const after = useQuickViewStore.getState().currentKey();
      if (after !== null && after !== before) syncMainAnchor();
      if (after === null && before !== null) void closeViewer();
    });
  });
  useItemsStore.subscribe((state, previous) => {
    if (state.detail !== previous.detail || state.selectedItem !== previous.selectedItem) {
      broadcastViewer();
    }
  });
}

export function openViewerFromMain(
  presentation: ViewerPresentation,
  preferredMonitor?: ViewerMonitor,
): boolean {
  const items = useItemsStore.getState();
  if (items.selectedItem === null || items.selectedKeys.size === 0) {
    useItemsStore.setState({ message: "Select an item to open the viewer." });
    return false;
  }
  const displayed = sectionProjection(items.items, items.currentSort()).orderedItems;
  const session = createViewerSession(
    presentation,
    displayed.map((item) => ({ key: itemKey(item), pathId: item.pathId })),
    items.selectedKeys,
    items.selectedItem,
  );
  if (session === null) {
    useItemsStore.setState({ message: "The selected item is no longer available." });
    return false;
  }
  useQuickViewStore.getState().start(session);
  if (presentation === "fullscreen") {
    beginFullscreen("main", preferredMonitor);
  }
  return true;
}

export function handleSpaceQuickView(event: {
  preventDefault: () => void;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): boolean {
  if (event.metaKey || event.ctrlKey || event.altKey) return false;
  const opened = openViewerFromMain("quick");
  event.preventDefault();
  return opened;
}

export function handleFViewer(event: {
  preventDefault: () => void;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): boolean {
  if (event.metaKey || event.ctrlKey || event.altKey) return false;
  const opened = openViewerFromMain("fullscreen");
  event.preventDefault();
  return opened;
}

export function moveViewer(move: ViewerMove): void {
  const before = useQuickViewStore.getState().currentKey();
  useQuickViewStore.getState().move(move);
  if (useQuickViewStore.getState().currentKey() !== before) syncMainAnchor();
}

export async function setViewerPresentation(presentation: ViewerPresentation): Promise<void> {
  const current = useQuickViewStore.getState().session?.presentation;
  if (current === null || current === undefined || current === presentation) return;
  useQuickViewStore.getState().setPresentation(presentation);
  if (presentation === "fullscreen") {
    beginFullscreen("quick");
  } else {
    fullscreenRequest += 1;
    clearFullscreenSurface();
    await exitViewerFullscreen();
    await restoreMainFocus();
  }
}

export async function closeViewer(): Promise<void> {
  const presentation = useQuickViewStore.getState().session?.presentation;
  useQuickViewStore.getState().close();
  fullscreenRequest += 1;
  if (presentation === "fullscreen") {
    clearFullscreenSurface();
    await exitViewerFullscreen();
  }
  await restoreMainFocus();
}

export async function requestViewerDelete(permanent: boolean): Promise<void> {
  const configConfirms = useAppStore.getState().appData?.config?.confirmTrashDelete === true;
  if (permanent || configConfirms) {
    useQuickViewStore.getState().requestDelete(permanent ? "permanent" : "trash");
    return;
  }
  await deleteViewerCurrent(false);
}

export async function confirmViewerDelete(): Promise<void> {
  const kind = useQuickViewStore.getState().pendingDelete;
  useQuickViewStore.getState().cancelDelete();
  if (kind !== null) await deleteViewerCurrent(kind === "permanent");
}

async function deleteViewerCurrent(permanent: boolean): Promise<void> {
  const key = useQuickViewStore.getState().currentKey();
  if (key !== null) await deleteItems(new Set([key]), permanent);
}

export async function handleViewerKey(message: ViewerKeyMessage): Promise<void> {
  if (message.metaKey || message.ctrlKey || message.altKey) return;
  const session = useQuickViewStore.getState().session;
  if (session === null) return;
  if (message.key === "Escape") {
    await closeViewer();
  } else if (message.key === " ") {
    if (session.presentation === "fullscreen") await setViewerPresentation("quick");
    else await closeViewer();
  } else if (message.key.toLowerCase() === "f") {
    if (session.presentation === "fullscreen") await closeViewer();
    else await setViewerPresentation("fullscreen");
  } else if (message.key === "ArrowLeft") {
    moveViewer("previous");
  } else if (message.key === "ArrowRight") {
    moveViewer("next");
  } else if (
    message.key === "PageUp" &&
    useItemsStore.getState().selected?.kind !== "other"
  ) {
    moveViewer("previous");
  } else if (
    message.key === "PageDown" &&
    useItemsStore.getState().selected?.kind !== "other"
  ) {
    moveViewer("next");
  } else if (message.key === "Home" && useItemsStore.getState().selected?.kind !== "other") {
    moveViewer("first");
  } else if (message.key === "End" && useItemsStore.getState().selected?.kind !== "other") {
    moveViewer("last");
  } else if (message.key === "Enter") {
    const item = currentItem();
    const kind = useItemsStore.getState().selected?.kind;
    if (item !== null && (kind === "video" || isAudioFile(item.fileName))) {
      toggleMainPlayback(itemKey(item));
    }
  } else if (message.key === "Delete" || message.key === "Backspace") {
    await requestViewerDelete(message.shiftKey === true);
  }
}
