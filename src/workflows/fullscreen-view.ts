// The fullscreen view is one application-owned session shown in a reusable
// borderless fullscreen window. Unlike the preview, which follows Main's
// selection, it holds the list while it is open and shows one item at a time.
// The native disk-backed sequence owns frozen membership and order; the window
// owns no library state.

import { emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ViewerMove, ViewerSequenceSnapshot } from "../models/viewerSession";
import { viewerMainIndex } from "../models/viewerSession";
import type { ItemDetail, SectionItem, SectionLocation } from "../models/items";
import { identityFromKey, identityKey, isAudioFile, itemKey } from "../models/items";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import {
  latestActivityOperationId,
  newActivityOperationId,
  recordActivity,
} from "../repositories/activity";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { beginMainFeedback } from "../state/main-feedback-store";
import { useFullscreenViewStore, type FullscreenViewDeleteReview } from "../state/fullscreen-view-store";
import { recordActionFailure } from "../state/notifications-store";
import { deleteItems } from "./items";
import { toggleMainPlayback } from "./playback";
import { showFullscreenViewWindow, hideFullscreenViewWindow } from "./fullscreen-view-window";
import { createEventInstaller } from "../utils/eventInstallation";
import { useComparisonStore } from "../state/comparison-store";
import { message, type Message } from "../i18n/translate";
import { confirmsTrashDelete } from "../models/config";

export interface FullscreenViewBroadcast {
  item: SectionItem | null;
  detail: ItemDetail | null;
  index: number;
  length: number;
  pendingDelete: Pick<FullscreenViewDeleteReview, "kind" | "fileName"> | null;
  sectionKind: "image" | "video" | "other" | null;
  /** A descriptor, not words: the fullscreen window renders it in its own
   * language, and follows a language change while it stays on screen. */
  failure: Message | null;
}

interface FullscreenViewKeyMessage {
  key: string;
  repeat?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}

let itemReconcileQueued = false;
let viewerOpenRequest = 0;
let viewerSequenceQueue: Promise<void> = Promise.resolve();

function enqueueViewerSequence(task: () => Promise<void>): void {
  viewerSequenceQueue = viewerSequenceQueue.then(task, task);
}

function currentItem(): SectionItem | null {
  return useFullscreenViewStore.getState().session?.item ?? null;
}

export function fullscreenViewBroadcast(): FullscreenViewBroadcast {
  const session = useFullscreenViewStore.getState().session;
  const item = currentItem();
  return {
    item,
    detail: session?.detail ?? null,
    index: session?.index ?? 0,
    length: session?.length ?? 0,
    pendingDelete: (() => {
      const pending = useFullscreenViewStore.getState().pendingDelete;
      return pending === null ? null : { kind: pending.kind, fileName: pending.fileName };
    })(),
    sectionKind: session === null ? null
      : session.detail.kind === "image" ? "image"
      : session.detail.kind === "video" ? "video" : "other",
    failure: useFullscreenViewStore.getState().failure,
  };
}

/** Closing broadcasts the empty state too, so the window is blank before it
 * hides and never flashes the previous item when it is shown again. */
function broadcastFullscreenView(): void {
  void emit("fullscreen-view://state", fullscreenViewBroadcast()).catch(
    reportWindowCall("fullscreen view state broadcast"),
  );
}

async function syncMainAnchor(): Promise<void> {
  const viewer = useFullscreenViewStore.getState();
  const key = viewer.currentKey();
  const session = viewer.session;
  if (key === null || session === null) return;
  const before = useItemsStore.getState();
  const sort = before.currentSort();
  const position = viewerMainIndex(session, {
    section: before.selected, sort, revision: before.reconciliationId,
    loading: before.loading, positions: before.itemPositions,
  });
  const subsetMatches = session.main.selectedKeys.length === before.selectedKeys.size &&
    session.main.selectedKeys.every((member) => before.selectedKeys.has(member));
  if (position !== null && (session.scope === "section" || subsetMatches)) {
    if (session.scope === "selection") before.setAnchor(key, position);
    else before.selectItem(key, "nearest", position);
    return;
  }
  const ownsViewer = () => useFullscreenViewStore.getState().session?.token === session.token &&
    useFullscreenViewStore.getState().currentKey() === key;
  let section: SectionLocation | null;
  try {
    section = await invoke<SectionLocation | null>("get_item_section", { identity: session.member });
  } catch (error) {
    log.error("fullscreen view Main location failed", toErrorFields(error));
    if (ownsViewer()) {
      const failure = message("fullscreenView.locateInMainFailed");
      useFullscreenViewStore.getState().setFailure(failure);
      recordActionFailure("fullscreen-view-main-location-failed", failure, error);
    }
    return;
  }
  const current = useItemsStore.getState();
  if (!ownsViewer() || current.selected !== before.selected ||
    current.selectedKeys !== before.selectedKeys || current.selectedItem !== before.selectedItem ||
    current.currentSort().order !== sort.order || current.currentSort().desc !== sort.desc) return;
  if (section === null) {
    useFullscreenViewStore.getState().setFailure(message("fullscreenView.notInMain"));
    return;
  }
  await current.select(section, {
    anchor: key, context: null,
    selectedKeys: session.scope === "selection" ? session.main.selectedKeys : [],
  });
  const restored = useItemsStore.getState();
  if (ownsViewer() && !restored.loading && restored.loadError === null && restored.selectedItem === key) {
    useFullscreenViewStore.getState().attachMainProjection({
      section: restored.selected, sort: restored.currentSort(), revision: restored.reconciliationId,
    });
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

/** A window failure closes only the session it was shown for: a later
 * session opened in the meantime keeps its own window. */
function showWindow(token: string, feedback: ReturnType<typeof beginMainFeedback>): void {
  void showFullscreenViewWindow()
    .then(() => {
      broadcastFullscreenView();
      feedback.finish();
    })
    .catch((error) => {
      log.error("fullscreen view window failed", toErrorFields(error));
      if (useFullscreenViewStore.getState().session?.token !== token) return;
      void closeFullscreenView();
      const failure = message("fullscreenView.showFailed");
      feedback.finish({ tone: "danger", text: failure });
      recordActionFailure("fullscreen-open-failed", failure, error);
    });
}

const install = createEventInstaller(
  async (listeners) => {
    await listeners.listen("fullscreen-view://ready", broadcastFullscreenView);
    await listeners.listen<FullscreenViewKeyMessage>("fullscreen-view://key", (event) => {
      void handleFullscreenViewKey(event.payload);
    });
    await listeners.listen("fullscreen-view://confirm-delete", () => {
      void confirmFullscreenViewDelete();
    });
    await listeners.listen("fullscreen-view://cancel-delete", () => {
      useFullscreenViewStore.getState().cancelDelete();
    });
    await listeners.listen("fullscreen-view://dismiss-failure", () => {
      useFullscreenViewStore.getState().setFailure(null);
    });
    // Switching to another app closes the view: Main's selection has followed
    // its navigation, so one Space reopens the same item. Focus stays with
    // the app the user switched to.
    await listeners.listen<boolean>("app://activation", (event) => {
      if (!event.payload && useFullscreenViewStore.getState().session !== null) {
        void closeFullscreenView({ restoreFocus: false });
      }
    });
    listeners.retain(useFullscreenViewStore.subscribe((state, previous) => {
      if (state.session !== null || previous.session !== null) broadcastFullscreenView();
    }));
    listeners.retain(useItemsStore.subscribe((state, previous) => {
      if (state.reconciliationId === previous.reconciliationId || itemReconcileQueued) return;
      itemReconcileQueued = true;
      queueMicrotask(() => {
        itemReconcileQueued = false;
        void reconcileViewerSequence();
      });
    }));
  },
  (error) => log.error("fullscreen view workflow wiring failed", toErrorFields(error)),
);

/** Installs the cross-window handshake and disappearance reconciliation once. */
export function installFullscreenViewWorkflow(): Promise<void> {
  return install();
}

async function reconcileViewerSequence(): Promise<void> {
  const session = useFullscreenViewStore.getState().session;
  if (session === null) return;
  enqueueViewerSequence(async () => {
    const current = useFullscreenViewStore.getState().session;
    if (current?.token !== session.token) return;
    const before = identityKey(current.member);
    try {
      const snapshot = await invoke<ViewerSequenceSnapshot | null>(
        "viewer_sequence_reconcile",
        { token: session.token },
      );
      if (useFullscreenViewStore.getState().session?.token !== session.token) return;
      if (snapshot === null) {
        await closeFullscreenView();
        return;
      }
      useFullscreenViewStore.getState().update(snapshot);
      useFullscreenViewStore.getState().setFailure(null);
      if (identityKey(snapshot.member) !== before) await syncMainAnchor();
    } catch (error) {
      log.error("viewer sequence reconciliation failed", toErrorFields(error));
      if (useFullscreenViewStore.getState().session?.token !== session.token) return;
      const failure = message("fullscreenView.refreshFailed");
      useFullscreenViewStore.getState().setFailure(failure);
      recordActionFailure("fullscreen-view-reconcile-failed", failure, error);
    }
  });
}

/** Space or double-click on Main's list: the fullscreen view opens on the
 * anchor, within the selection when more than one item is selected. */
export function openFullscreenView(): boolean {
  if (useComparisonStore.getState().open) return false;
  const feedback = beginMainFeedback("fullscreen-view");
  const items = useItemsStore.getState();
  if (items.selectedItem === null || items.selectedKeys.size === 0) {
    feedback.finish({ tone: "normal", text: message("fullscreenView.selectItemFirst") });
    return false;
  }
  const section = items.selected;
  const loadedPositions = new Map(
    items.items.map((item, offset) => [itemKey(item), items.windowStart + offset]),
  );
  const anchorPosition = items.selectedPositions.get(items.selectedItem) ?? loadedPositions.get(items.selectedItem);
  if (section === null || anchorPosition === undefined) {
    feedback.finish({ tone: "normal", text: message("fullscreenView.selectionGone") });
    return false;
  }
  const request = ++viewerOpenRequest;
  const owner = "fullscreen";
  const operationId = newActivityOperationId(owner);
  recordActivity({
    kind: "started",
    owner,
    operationId,
    causeId: latestActivityOperationId("selection"),
    current: "running",
    reason: "user",
    lane: section.kind,
    itemCount: items.selectedKeys.size,
  });
  const selected = [...items.selectedKeys].flatMap((key) => {
    const index = items.selectedPositions.get(key) ?? loadedPositions.get(key);
    return index === undefined ? [] : [{ ...identityFromKey(key), index }];
  });
  const entrySort = items.currentSort();
  void invoke<ViewerSequenceSnapshot>("viewer_sequence_start", {
    kind: section.kind,
    month: section.month,
    sort: entrySort,
    selected,
    anchor: identityFromKey(items.selectedItem),
  })
    .then((snapshot) => {
      if (request !== viewerOpenRequest) {
        recordActivity({
          kind: "stale",
          owner,
          operationId,
          previous: "running",
          current: "stale",
          reason: "superseded",
        });
        void invoke("viewer_sequence_close", { token: snapshot.token }).catch((error) =>
          log.warn("stale viewer sequence cleanup failed", toErrorFields(error)),
        );
        return;
      }
      useFullscreenViewStore.getState().start(snapshot, {
        projection: { section, sort: entrySort, revision: items.reconciliationId },
        selectedKeys: [...items.selectedKeys], frozenPositionsValid: true,
      });
      recordActivity({
        kind: "opened",
        owner,
        operationId,
        previous: "running",
        current: "open",
        reason: "completion",
      });
      showWindow(snapshot.token, feedback);
    })
    .catch((error) => {
      if (request !== viewerOpenRequest) return;
      log.error("viewer sequence start failed", toErrorFields(error));
      const failure = message("fullscreenView.openFailed");
      feedback.finish({ tone: "danger", text: failure });
      recordActionFailure("fullscreen-view-open-failed", failure, error);
      recordActivity({
        kind: "failed",
        owner,
        operationId,
        previous: "running",
        current: "failed",
        reason: "error",
      });
    });
  return true;
}

export function handleSpaceFullscreenView(event: {
  preventDefault: () => void;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): boolean {
  if (event.metaKey || event.ctrlKey || event.altKey) return false;
  const opened = openFullscreenView();
  event.preventDefault();
  return opened;
}

export function moveFullscreenView(move: ViewerMove): void {
  const session = useFullscreenViewStore.getState().session;
  if (session === null) return;
  enqueueViewerSequence(async () => {
    if (useFullscreenViewStore.getState().session?.token !== session.token) return;
    try {
      const snapshot = await invoke<ViewerSequenceSnapshot>("viewer_sequence_move", {
        token: session.token,
        movement: move,
      });
      if (useFullscreenViewStore.getState().session?.token !== session.token) return;
      useFullscreenViewStore.getState().update(snapshot);
      useFullscreenViewStore.getState().setFailure(null);
      await syncMainAnchor();
    } catch (error) {
      log.error("fullscreen view navigation failed", toErrorFields(error));
      if (useFullscreenViewStore.getState().session?.token !== session.token) return;
      const failure = message("fullscreenView.navigationFailed");
      useFullscreenViewStore.getState().setFailure(failure);
      recordActionFailure("fullscreen-view-navigation-failed", failure, error);
    }
  });
}

export async function closeFullscreenView({ restoreFocus = true } = {}): Promise<void> {
  viewerOpenRequest += 1;
  const session = useFullscreenViewStore.getState().session;
  if (session !== null) {
    recordActivity({
      kind: "closed",
      owner: "fullscreen",
      previous: "open",
      current: "closed",
      reason: "user",
    });
  }
  useFullscreenViewStore.getState().close();
  if (session !== null) {
    await invoke("viewer_sequence_close", { token: session.token }).catch((error) =>
      log.warn("viewer sequence cleanup failed", toErrorFields(error)),
    );
    await hideFullscreenViewWindow();
  }
  // Library updates and mutations own Main reconciliation. Closing the view
  // must not replace an already-admitted Main navigation with a second load.
  if (restoreFocus) await restoreMainFocus();
  else focusMainAnchor();
}

export async function requestFullscreenViewDelete(permanent: boolean): Promise<void> {
  const configConfirms = confirmsTrashDelete(useAppStore.getState().appData?.config);
  if (permanent || configConfirms) {
    useFullscreenViewStore.getState().requestDelete(permanent ? "permanent" : "trash");
    return;
  }
  await deleteFullscreenViewCurrent(false);
}

export async function confirmFullscreenViewDelete(): Promise<void> {
  const pending = useFullscreenViewStore.getState().pendingDelete;
  useFullscreenViewStore.getState().cancelDelete();
  if (pending !== null) await deleteItems([pending.key], pending.kind === "permanent");
}

async function deleteFullscreenViewCurrent(permanent: boolean): Promise<void> {
  const key = useFullscreenViewStore.getState().currentKey();
  if (key !== null) await deleteItems([key], permanent);
}

export async function handleFullscreenViewKey(message: FullscreenViewKeyMessage): Promise<void> {
  if (message.metaKey || message.ctrlKey || message.altKey) return;
  const session = useFullscreenViewStore.getState().session;
  if (session === null || useFullscreenViewStore.getState().pendingDelete !== null) return;
  if (message.repeat && ["Escape", " ", "Enter", "Delete", "Backspace"].includes(message.key)) return;
  const sequenceBounds = session.detail.kind !== "other" || isAudioFile(session.item.fileName);
  if (message.key === "Escape" || message.key === " ") {
    await closeFullscreenView();
  } else if (message.key === "ArrowLeft") {
    moveFullscreenView("previous");
  } else if (message.key === "ArrowRight") {
    moveFullscreenView("next");
  } else if (
    message.key === "PageUp" &&
    sequenceBounds
  ) {
    moveFullscreenView("previous");
  } else if (
    message.key === "PageDown" &&
    sequenceBounds
  ) {
    moveFullscreenView("next");
  } else if (message.key === "Home" && sequenceBounds) {
    moveFullscreenView("first");
  } else if (message.key === "End" && sequenceBounds) {
    moveFullscreenView("last");
  } else if (message.key === "Enter") {
    const item = currentItem();
    const kind = session.detail.kind;
    if (item !== null && (kind === "video" || isAudioFile(item.fileName))) {
      toggleMainPlayback(itemKey(item));
    }
  } else if (message.key === "Delete" || message.key === "Backspace") {
    if (message.repeat === true) return;
    await requestFullscreenViewDelete(message.shiftKey === true);
  }
}
