// Comparison's window/page/commit state machine remains cohesive in its
// store. This application edge owns every cross-store consequence: Preview
// handoff, app preferences, item/count refresh, Issues, and chain selection.

import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import {
  broadcastComparison,
  type ComparisonCommitResult,
  slotIndexForKey,
  slotIndexForShiftedCode,
  useComparisonStore,
} from "../state/comparison-store";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import {
  hidePreviewForComparison,
  restorePreviewAfterComparison,
} from "../state/preview-store";
import { useSectionsStore } from "../state/sections-store";
import { hasOpenModal } from "../utils/modalStack";

let eventInstallation: Promise<void> | null = null;

function appState(): Record<string, unknown> {
  return useAppStore.getState().appData?.state ?? {};
}

function configConfirmsTrash(): boolean {
  return useAppStore.getState().appData?.config?.confirmTrashDelete === true;
}

async function refreshLibrary(): Promise<void> {
  await Promise.all([
    useItemsStore.getState().refresh(),
    useSectionsStore.getState().loadCounts(),
  ]);
}

async function applyCommitResult(
  result: ComparisonCommitResult | null,
): Promise<void> {
  if (result === null) return;
  if (result.kind === "failed") {
    await Promise.all([refreshLibrary(), useIssuesStore.getState().load()]);
    return;
  }
  await restorePreviewAfterComparison();
  await refreshLibrary();
  // Land after the decided family so Enter chains into the next one.
  useItemsStore.getState().selectAfterFamily(result.family);
}

export async function openComparison(hash: string): Promise<boolean> {
  await hidePreviewForComparison();
  const opened = await useComparisonStore.getState().openGroup(hash, appState());
  if (!opened && !useComparisonStore.getState().open) {
    await restorePreviewAfterComparison();
  }
  return opened;
}

export async function closeComparison(): Promise<void> {
  await useComparisonStore.getState().close();
  await restorePreviewAfterComparison();
  await refreshLibrary();
}

export async function commitComparison(permanent: boolean): Promise<void> {
  const result = await useComparisonStore
    .getState()
    .commitTurn(permanent, configConfirmsTrash());
  await applyCommitResult(result);
}

export async function confirmComparisonCommit(): Promise<void> {
  await applyCommitResult(
    await useComparisonStore.getState().confirmPendingCommit(),
  );
}

export async function confirmPermanentComparisonCommit(): Promise<void> {
  await applyCommitResult(
    await useComparisonStore
      .getState()
      .confirmPermanentCommit(configConfirmsTrash()),
  );
}

async function installEvents(): Promise<void> {
  try {
    if (getCurrentWindow().label !== "main") return;
    await listen<{
      key: string;
      code?: string;
      shiftKey: boolean;
      metaKey?: boolean;
      ctrlKey?: boolean;
      altKey?: boolean;
    }>("comparison://key", (event) => {
      const store = useComparisonStore.getState();
      if (!store.open || hasOpenModal()) return;
      const unlinkIndex = slotIndexForShiftedCode(event.payload);
      if (unlinkIndex >= 0) {
        void store.unlinkSlot(unlinkIndex);
        return;
      }
      const slotIndex = slotIndexForKey(event.payload);
      if (slotIndex >= 0) {
        store.toggleKeep(slotIndex);
      } else if (event.payload.key === "Enter") {
        void commitComparison(event.payload.shiftKey);
      } else if (event.payload.key === "Escape") {
        void closeComparison();
      } else if (
        event.payload.key === "ArrowRight" ||
        event.payload.key === "PageDown"
      ) {
        store.nextPage();
      } else if (
        event.payload.key === "ArrowLeft" ||
        event.payload.key === "PageUp"
      ) {
        store.prevPage();
      } else if (event.payload.key.toLowerCase() === "s") {
        store.toggleShortlist();
      }
    });
    await listen("comparison://ready", () => {
      if (useComparisonStore.getState().open) broadcastComparison();
    });
  } catch (error) {
    log.warn("comparison spread wiring failed", toErrorFields(error));
  }
}

export function installComparisonEventWiring(): Promise<void> {
  eventInstallation ??= installEvents();
  return eventInstallation;
}
