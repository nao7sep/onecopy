import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { comparisonHashForSelection } from "../models/interactions";
import { sectionProjection } from "../models/items";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import {
  broadcastComparison,
  recoverComparisonDisplay,
  slotIndexForKey,
  useComparisonStore,
  visibleMembers,
  type ComparisonCommitResult,
  type ComparisonOpenResult,
} from "../state/comparison-store";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import { useMutationStore } from "../state/mutation-store";
import { restorePreviewAfterComparison } from "../state/preview-store";
import { useSectionsStore } from "../state/sections-store";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { hasOpenModal } from "../utils/modalStack";

let eventInstallation: Promise<void> | null = null;

function appState(): Record<string, unknown> {
  return useAppStore.getState().appData?.state ?? {};
}

function configConfirmsTrash(): boolean {
  return useAppStore.getState().appData?.config?.confirmTrashDelete === true;
}

function maximumImages(): number {
  const value =
    useAppStore.getState().appData?.config?.maximumImagesInComparison;
  return typeof value === "number" && Number.isFinite(value)
    ? Math.max(2, Math.floor(value))
    : 16;
}

async function refreshLibrary(): Promise<void> {
  await Promise.all([
    useItemsStore.getState().refresh(),
    useSectionsStore.getState().loadCounts(),
  ]);
}

async function applyResult(
  result: ComparisonCommitResult | null,
): Promise<void> {
  if (result === null) return;
  const orderBeforeComparison =
    result.kind === "completed" ? useItemsStore.getState().items : null;
  await Promise.all([refreshLibrary(), useIssuesStore.getState().load()]);
  if (result.kind === "failed") {
    await reconcileComparisonMembership();
    return;
  }
  if (result.kind === "continued") {
    await reconcileComparisonMembership();
    return;
  }
  useItemsStore
    .getState()
    .selectAfterFamily(result.family, orderBeforeComparison ?? []);
  await restorePreviewAfterComparison();
}

export async function openComparison(
  hash: string,
  initialSelection: Iterable<string> = [hash],
  entryAnchor: string | null = hash,
): Promise<ComparisonOpenResult> {
  return await useComparisonStore
    .getState()
    .openGroup(
      hash,
      initialSelection,
      entryAnchor,
      maximumImages(),
      appState(),
    );
}

export async function requestComparisonFromMain(): Promise<void> {
  const { selected, items, selectedKeys, selectedItem } =
    useItemsStore.getState();
  const state = useItemsStore.getState();
  const projection = sectionProjection(items, state.currentSort());
  const selectedItems = [...selectedKeys].flatMap((key) => {
    const item = projection.itemByKey.get(key);
    return item === undefined ? [] : [item];
  });
  const hash =
    selected?.kind === "image"
      ? comparisonHashForSelection(selectedItems, selectedKeys, selectedItem)
      : null;
  if (hash === null) {
    useItemsStore.setState({
      message: "Comparison requires images from one similar group.",
    });
    return;
  }
  const result = await openComparison(hash, selectedKeys, selectedItem);
  if (result === "unavailable") {
    useItemsStore.setState({
      message: "There are no similar images left to compare.",
    });
  } else if (result === "failed") {
    useItemsStore.setState({
      message: "Couldn’t open Comparison. See Issues for details.",
    });
  }
}

export async function closeComparison(): Promise<void> {
  if (useComparisonStore.getState().busy) {
    await useMutationStore.getState().cancel();
    return;
  }
  await useComparisonStore.getState().close();
  await refreshLibrary();
  await restorePreviewAfterComparison();
}

export async function decideComparisonPage(
  permanent: boolean,
  trashAll = false,
): Promise<void> {
  await applyResult(
    await useComparisonStore
      .getState()
      .requestPageDecision(permanent, configConfirmsTrash(), trashAll),
  );
}

export async function deleteComparisonSelection(
  permanent: boolean,
): Promise<void> {
  await applyResult(
    await useComparisonStore
      .getState()
      .requestSelectionDelete(permanent, configConfirmsTrash()),
  );
}

export async function confirmComparisonAction(): Promise<void> {
  await applyResult(await useComparisonStore.getState().confirmPendingAction());
}

export async function retryComparisonFailure(): Promise<void> {
  await applyResult(
    await useComparisonStore.getState().retryFailure(configConfirmsTrash()),
  );
}

export async function unlinkComparisonSelection(): Promise<void> {
  const result = await useComparisonStore.getState().unlinkSelected();
  if (result === null) return;
  await Promise.all([refreshLibrary(), useIssuesStore.getState().load()]);
  if (result === "closed") {
    await restorePreviewAfterComparison();
  } else {
    await reconcileComparisonMembership();
  }
}

export async function reconcileComparisonMembership(): Promise<void> {
  const store = useComparisonStore.getState();
  if (!store.open || store.busy) return;
  const sessionId = store.sessionId;
  const requested = store.members.map((member) => member.hash);
  try {
    const live = await invoke<string[]>("comparison_live_hashes", {
      hashes: requested,
    });
    const current = useComparisonStore.getState();
    if (!current.open || current.busy || current.sessionId !== sessionId) return;
    const stillOpen = await current.reconcileLiveMembers(live);
    if (!stillOpen) {
      await refreshLibrary();
      await restorePreviewAfterComparison();
    }
  } catch (error) {
    log.warn("comparison membership refresh failed", toErrorFields(error));
    recordInterfaceFailure("Couldn’t check recent file changes in Comparison.");
    useComparisonStore.setState({
      message: "Couldn’t check recent file changes in Comparison.",
    });
  }
}

export function comparisonKeyIsRoutable(event: {
  key: string;
  repeat?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}, visibleCount = 36): boolean {
  const directIndex = slotIndexForKey(event);
  if (directIndex >= 0) return directIndex < visibleCount;
  const command = event.metaKey === true || event.ctrlKey === true;
  if (
    command &&
    event.altKey !== true &&
    event.shiftKey !== true &&
    event.key.toLowerCase() === "a"
  ) {
    return true;
  }
  if (command || event.altKey === true) return false;
  if (
    event.key === "Enter" ||
    event.key === "Delete" ||
    event.key === "Backspace" ||
    event.key === "Home" ||
    event.key === "End" ||
    event.key.startsWith("Arrow")
  ) {
    return true;
  }
  if (event.shiftKey === true) return false;
  return (
    event.key === "Escape" ||
    event.key === "PageDown" ||
    event.key === "PageUp" ||
    event.key === " "
  );
}

export function handleComparisonKey(event: {
  key: string;
  repeat?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): boolean {
  const store = useComparisonStore.getState();
  if (!store.open || hasOpenModal()) return false;
  if (store.busy) {
    if (
      event.key === "Escape" &&
      event.metaKey !== true &&
      event.ctrlKey !== true &&
      event.altKey !== true &&
      event.shiftKey !== true
    ) {
      void useMutationStore.getState().cancel();
      return true;
    }
    return false;
  }
  if (!comparisonKeyIsRoutable(event, visibleMembers(store).length)) {
    return false;
  }
  const slotIndex = slotIndexForKey(event);
  if (slotIndex >= 0) {
    store.selectSlot(slotIndex, "toggle");
    return true;
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
    store.selectAll();
    return true;
  }
  if (event.key === "Enter") {
    void decideComparisonPage(event.shiftKey === true);
    return true;
  }
  if (event.key === "Delete" || event.key === "Backspace") {
    void deleteComparisonSelection(event.shiftKey === true);
    return true;
  }
  if (event.key === "Escape") {
    void closeComparison();
    return true;
  }
  if (event.key === "PageDown") {
    store.nextPage();
    return true;
  }
  if (event.key === "PageUp") {
    store.prevPage();
    return true;
  }
  if (event.key === "Home" || event.key === "End") {
    store.selectBound(
      event.key === "Home" ? "first" : "last",
      event.shiftKey === true,
    );
    return true;
  }
  if (event.key.startsWith("Arrow")) {
    const direction = event.key.slice(5).toLowerCase();
    if (
      direction === "left" ||
      direction === "right" ||
      direction === "up" ||
      direction === "down"
    ) {
      store.moveSelection(direction, event.shiftKey === true);
      return true;
    }
  }
  if (event.key === " ") return true;
  return false;
}

async function installEvents(): Promise<void> {
  try {
    if (getCurrentWindow().label !== "main") return;
    await listen<{
      key: string;
      repeat?: boolean;
      shiftKey?: boolean;
      metaKey?: boolean;
      ctrlKey?: boolean;
      altKey?: boolean;
    }>("comparison://key", (event) => {
      handleComparisonKey(event.payload);
    });
    await listen<{
      slotIndex: number;
      mode: "exclusive" | "toggle" | "range";
      decide?: boolean;
    }>("comparison://select", (event) => {
      const store = useComparisonStore.getState();
      if (!store.open || store.busy || hasOpenModal()) return;
      store.selectSlot(event.payload.slotIndex, event.payload.mode);
      if (event.payload.decide === true) void decideComparisonPage(false);
    });
    await listen("comparison://ready", () => {
      broadcastComparison();
    });
    await listen<{ slice: number }>("comparison://display-failed", (event) => {
      void recoverComparisonDisplay(event.payload.slice);
    });
  } catch (error) {
    log.warn("comparison display wiring failed", toErrorFields(error));
    const message = error instanceof Error ? error.message : String(error);
    recordInterfaceFailure(message);
    useItemsStore.setState({
      message:
        "Comparison-display controls are unavailable. Restart OneCopy to repair them.",
    });
  }
}

export function installComparisonEventWiring(): Promise<void> {
  eventInstallation ??= installEvents();
  return eventInstallation;
}
