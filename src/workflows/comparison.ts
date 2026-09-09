import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  anchorContextFromPayload,
  type AnchorContext,
} from "../models/mainSelection";
import type { SectionRecoveryContextPayload } from "../models/items";
import { visibleKeepMarks } from "../models/comparisonSession";
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
import { createEventInstaller } from "../utils/eventInstallation";
import { hasOpenModal } from "../utils/modalStack";
import { focusComparison, installComparisonImageEvents, openComparisonImage } from "./comparison-image";
import type { MonitorRect } from "../utils/windowBounds";
import {
  latestActivityOperationId,
  newActivityOperationId,
  recordActivity,
} from "../repositories/activity";

let mainRecoveryAfterFamily: AnchorContext | null = null;

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

async function restoreMainFocus(): Promise<void> {
  await getCurrentWindow().setFocus();
  document.getElementById("main-item-area")?.focus();
}

async function applyResult(
  result: ComparisonCommitResult | null,
): Promise<void> {
  if (result === null) return;
  await Promise.all([refreshLibrary(), useIssuesStore.getState().load()]);
  if (result.kind === "failed") {
    await reconcileComparisonMembership();
    return;
  }
  if (result.kind === "continued") {
    await reconcileComparisonMembership();
    return;
  }
  await useItemsStore.getState().selectAfterFamily(mainRecoveryAfterFamily);
  mainRecoveryAfterFamily = null;
  await restorePreviewAfterComparison();
  await restoreMainFocus();
}

export async function openComparison(
  hash: string,
  entryAnchor: string | null = hash,
): Promise<ComparisonOpenResult> {
  const result = await useComparisonStore
    .getState()
    .openGroup(
      hash,
      entryAnchor,
      maximumImages(),
      appState(),
    );
  if (result === "opened") {
    await focusComparison();
    const items = useItemsStore.getState();
    const section = items.selected;
    const hashes = useComparisonStore.getState().originalMemberHashes;
    if (section !== null) {
      try {
        const context = await invoke<SectionRecoveryContextPayload | null>(
          "get_section_family_context",
          {
            kind: section.kind,
            month: section.month,
            sort: items.currentSort(),
            memberHashes: hashes,
          },
        );
        mainRecoveryAfterFamily = anchorContextFromPayload(context);
      } catch (error) {
        log.error("comparison return position failed", toErrorFields(error));
        await useComparisonStore.getState().close();
        recordInterfaceFailure("Couldn’t prepare Comparison.");
        return "failed";
      }
    }
  }
  return result;
}

export async function requestComparisonFromMain(): Promise<void> {
  const { selected, selectedKeys, selectedItem } = useItemsStore.getState();
  const hashes = [...selectedKeys].filter((key) => !key.startsWith("path-"));
  const hash = selectedItem !== null && !selectedItem.startsWith("path-") ? selectedItem : null;
  if (
    selected?.kind !== "image" ||
    hash === null ||
    hashes.length !== selectedKeys.size
  ) {
    useItemsStore.setState({
      message: "Comparison requires images from one similar group.",
    });
    return;
  }
  const operationId = newActivityOperationId("comparison");
  recordActivity({
    kind: "started",
    owner: "comparison",
    operationId,
    causeId: latestActivityOperationId("selection"),
    current: "running",
    reason: "user",
    lane: "image",
    itemCount: selectedKeys.size,
  });
  try {
    const valid = await invoke<boolean>("comparison_selection_valid", { hashes });
    if (!valid) {
      useItemsStore.setState({
        message: "Comparison requires images from one similar group.",
      });
      recordActivity({
        kind: "completed",
        owner: "comparison",
        operationId,
        previous: "running",
        current: "idle",
        reason: "dependency",
      });
      return;
    }
  } catch (error) {
    log.error("comparison admission failed", toErrorFields(error));
    useItemsStore.setState({ message: "Couldn’t check the selected images." });
    recordActivity({
      kind: "failed",
      owner: "comparison",
      operationId,
      previous: "running",
      current: "failed",
      reason: "error",
    });
    return;
  }
  const result = await openComparison(hash, selectedItem);
  if (result === "unavailable") {
    useItemsStore.setState({
      message: "There are no similar images left to compare.",
    });
    recordActivity({
      kind: "completed",
      owner: "comparison",
      operationId,
      previous: "running",
      current: "idle",
      reason: "dependency",
    });
  } else if (result === "failed") {
    useItemsStore.setState({
      message: "Couldn’t open Comparison. See Issues for details.",
    });
    recordActivity({
      kind: "failed",
      owner: "comparison",
      operationId,
      previous: "running",
      current: "failed",
      reason: "error",
    });
  } else {
    recordActivity({
      kind: "opened",
      owner: "comparison",
      operationId,
      previous: "running",
      current: "open",
      reason: "completion",
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
  await restoreMainFocus();
  recordActivity({
    kind: "closed",
    owner: "comparison",
    previous: "open",
    current: "closed",
    reason: "user",
  });
}

export async function decideComparisonPage(
  permanent: boolean,
  trashAll = false,
): Promise<void> {
  const state = useComparisonStore.getState();
  if (
    !trashAll &&
    visibleKeepMarks(state.selected, visibleMembers(state)).size === 0
  ) {
    await closeComparison();
    return;
  }
  await applyResult(
    await useComparisonStore
      .getState()
      .requestPageDecision(permanent, trashAll),
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
    await useComparisonStore
      .getState()
      .retryFailure(configConfirmsTrash()),
  );
}

export async function unlinkComparisonSelection(): Promise<void> {
  const result = await useComparisonStore.getState().unlinkSelected();
  if (result === null) return;
  await Promise.all([refreshLibrary(), useIssuesStore.getState().load()]);
  if (result === "closed") {
    await restorePreviewAfterComparison();
    await restoreMainFocus();
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
      await restoreMainFocus();
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
  returnWindow?: string;
  monitor?: MonitorRect;
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
  if (
    event.repeat === true &&
    (event.key === "Enter" ||
      event.key === "Delete" ||
      event.key === "Backspace" ||
      event.key === " ")
  ) {
    return true;
  }
  const slotIndex = slotIndexForKey(event);
  if (slotIndex >= 0) {
    store.selectSlot(slotIndex, "toggle");
    return true;
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
    store.markAll();
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
    store.activateBound(
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
      store.moveActive(direction, event.shiftKey === true);
      return true;
    }
  }
  if (event.key === " ") {
    void openComparisonImage(event.returnWindow, event.monitor);
    return true;
  }
  return false;
}

const installEvents = createEventInstaller(
  async (listeners) => {
    if (getCurrentWindow().label !== "main") return;
    await installComparisonImageEvents(listeners);
    await listeners.listen<{ slice: number; aspect: number }>("comparison://layout", (event) => {
      useComparisonStore.getState().setDisplayAspect(event.payload.slice, event.payload.aspect);
    });
    await listeners.listen<{
      key: string;
      repeat?: boolean;
      shiftKey?: boolean;
      metaKey?: boolean;
      ctrlKey?: boolean;
      altKey?: boolean;
      returnWindow?: string;
      monitor?: MonitorRect;
    }>("comparison://key", (event) => {
      handleComparisonKey(event.payload);
    });
    await listeners.listen<{
      slotIndex: number;
      mode: "activate" | "toggle" | "range";
    }>("comparison://select", (event) => {
      const store = useComparisonStore.getState();
      if (!store.open || store.busy || hasOpenModal()) return;
      store.selectSlot(event.payload.slotIndex, event.payload.mode);
    });
    await listeners.listen("comparison://ready", () => {
      broadcastComparison();
    });
    await listeners.listen<{ slice: number }>("comparison://display-failed", (event) => {
      void recoverComparisonDisplay(event.payload.slice);
    });
  },
  (error) => {
    log.warn("comparison display wiring failed", toErrorFields(error));
    recordInterfaceFailure(
      "Comparison-display controls are unavailable. Restart OneCopy to repair them.",
    );
    useItemsStore.setState({
      message:
        "Comparison-display controls are unavailable. Restart OneCopy to repair them.",
    });
  },
);

export function installComparisonEventWiring(): Promise<void> {
  return installEvents();
}
