// The transient viewer is one application-owned session with two renderings:
// an overlay in Main and a reusable borderless fullscreen window. This edge
// coordinates those surfaces. The native disk-backed sequence owns frozen
// membership and order; neither React surface owns library state.

import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  ViewerMove,
  ViewerPresentation,
  ViewerSequenceSnapshot,
} from "../models/viewerSession";
import type { ItemDetail, SectionItem } from "../models/items";
import { identityFromKey, identityKey, isAudioFile, itemKey } from "../models/items";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { useQuickViewStore } from "../state/quick-view-store";
import { recordActionFailure } from "../state/notifications-store";
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
  failure: string | null;
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
let viewerOpenRequest = 0;
let viewerSequenceQueue: Promise<void> = Promise.resolve();

function enqueueViewerSequence(task: () => Promise<void>): void {
  viewerSequenceQueue = viewerSequenceQueue.then(task, task);
}

function currentItem(): SectionItem | null {
  return useQuickViewStore.getState().session?.item ?? null;
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
    length: session?.length ?? 0,
    pendingDelete: useQuickViewStore.getState().pendingDelete,
    sectionKind: items.selected?.kind ?? null,
    failure: useQuickViewStore.getState().failure,
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
    failure: null,
  } satisfies ViewerBroadcast).catch(reportWindowCall("viewer clear broadcast"));
}

function syncMainAnchor(): void {
  const viewer = useQuickViewStore.getState();
  const key = viewer.currentKey();
  const session = viewer.session;
  if (key === null || session === null) return;
  if (viewer.session?.scope === "selection") {
    useItemsStore.getState().setAnchor(key, session.sectionIndex);
  } else {
    useItemsStore.getState().selectItem(key, "nearest", session.sectionIndex);
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
      void closeViewer();
    }
    if (fallback === "quick") {
      useQuickViewStore.getState().setFailure("Couldn’t open full screen.");
    } else {
      useItemsStore.setState({ message: "Couldn’t open full screen." });
    }
    recordActionFailure("fullscreen-open-failed", "Couldn’t open full screen.", error);
  }
  if (fallback === "quick") {
    clearFullscreenSurface();
    void exitViewerFullscreen().then(restoreMainFocus);
  }
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
    listen("viewer://dismiss-failure", () => {
      useQuickViewStore.getState().setFailure(null);
    }),
    listen<ViewerMonitor | null>("preview://fullscreen", (event) => {
      openViewerFromMain("fullscreen", event.payload ?? undefined);
    }),
  ]);
  useQuickViewStore.subscribe(broadcastViewer);
  useItemsStore.subscribe((state, previous) => {
    if (state.reconciliationId === previous.reconciliationId || itemReconcileQueued) return;
    itemReconcileQueued = true;
    queueMicrotask(() => {
      itemReconcileQueued = false;
      void reconcileViewerSequence();
    });
  });
  useItemsStore.subscribe((state, previous) => {
    if (state.detail !== previous.detail || state.selectedItem !== previous.selectedItem) {
      broadcastViewer();
    }
  });
}

async function reconcileViewerSequence(): Promise<void> {
  const session = useQuickViewStore.getState().session;
  if (session === null) return;
  enqueueViewerSequence(async () => {
    const current = useQuickViewStore.getState().session;
    if (current?.token !== session.token) return;
    const before = identityKey(current.member);
    try {
      const snapshot = await invoke<ViewerSequenceSnapshot | null>(
        "viewer_sequence_reconcile",
        { token: session.token },
      );
      if (useQuickViewStore.getState().session?.token !== session.token) return;
      if (snapshot === null) {
        await closeViewer();
        return;
      }
      useQuickViewStore.getState().update(snapshot);
      useQuickViewStore.getState().setFailure(null);
      if (identityKey(snapshot.member) !== before) syncMainAnchor();
    } catch (error) {
      log.error("viewer sequence reconciliation failed", toErrorFields(error));
      const message = "Couldn’t refresh the open viewer.";
      useQuickViewStore.getState().setFailure(message);
      recordActionFailure(
        "viewer-reconcile-failed",
        message,
        error,
      );
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
  const section = items.selected;
  const loadedPositions = new Map(
    items.items.map((item, offset) => [itemKey(item), items.windowStart + offset]),
  );
  const anchorPosition = items.selectedPositions.get(items.selectedItem) ?? loadedPositions.get(items.selectedItem);
  if (section === null || anchorPosition === undefined) {
    useItemsStore.setState({ message: "The selected item is no longer available." });
    return false;
  }
  const request = ++viewerOpenRequest;
  const selected = [...items.selectedKeys].flatMap((key) => {
    const index = items.selectedPositions.get(key) ?? loadedPositions.get(key);
    return index === undefined ? [] : [{ ...identityFromKey(key), index }];
  });
  void invoke<ViewerSequenceSnapshot>("viewer_sequence_start", {
    kind: section.kind,
    month: section.month,
    sort: items.currentSort(),
    selected,
    anchor: identityFromKey(items.selectedItem),
  })
    .then((snapshot) => {
      if (request !== viewerOpenRequest) {
        void invoke("viewer_sequence_close", { token: snapshot.token }).catch((error) =>
          log.warn("stale viewer sequence cleanup failed", toErrorFields(error)),
        );
        return;
      }
      useQuickViewStore.getState().start(snapshot, presentation);
      if (presentation === "fullscreen") beginFullscreen("main", preferredMonitor);
    })
    .catch((error) => {
      if (request !== viewerOpenRequest) return;
      log.error("viewer sequence start failed", toErrorFields(error));
      useItemsStore.setState({ message: "Couldn’t open the viewer." });
      recordActionFailure("viewer-open-failed", "Couldn’t open the viewer.", error);
    });
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
  const session = useQuickViewStore.getState().session;
  if (session === null) return;
  enqueueViewerSequence(async () => {
    if (useQuickViewStore.getState().session?.token !== session.token) return;
    try {
      const snapshot = await invoke<ViewerSequenceSnapshot>("viewer_sequence_move", {
        token: session.token,
        movement: move,
      });
      if (useQuickViewStore.getState().session?.token !== session.token) return;
      useQuickViewStore.getState().update(snapshot);
      useQuickViewStore.getState().setFailure(null);
      syncMainAnchor();
    } catch (error) {
      log.error("viewer navigation failed", toErrorFields(error));
      const message = "Couldn’t move in the viewer.";
      useQuickViewStore.getState().setFailure(message);
      recordActionFailure("viewer-navigation-failed", message, error);
    }
  });
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
  viewerOpenRequest += 1;
  const session = useQuickViewStore.getState().session;
  const presentation = session?.presentation;
  useQuickViewStore.getState().close();
  if (session !== null) {
    await invoke("viewer_sequence_close", { token: session.token }).catch((error) =>
      log.warn("viewer sequence cleanup failed", toErrorFields(error)),
    );
  }
  fullscreenRequest += 1;
  if (presentation === "fullscreen") {
    clearFullscreenSurface();
    await exitViewerFullscreen();
  }
  await useItemsStore.getState().refresh();
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
