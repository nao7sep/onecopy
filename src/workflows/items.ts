// Item journeys at the application edge. The item store owns selection and
// request sequencing; this module persists its public choices, projects the
// anchor into Preview, and coordinates mutations with Issues and counts.

import { invoke } from "@tauri-apps/api/core";
import { identityFromKey, itemKey } from "../models/items";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import { usePreviewStore } from "../state/preview-store";
import { useSectionsStore } from "../state/sections-store";
import { recordActionFailure } from "../state/notifications-store";

interface DeleteBatchOutcome {
  error: string | null;
  failedFiles: number;
}

let installed = false;

function appWindowState(): Record<string, unknown> {
  return useAppStore.getState().appData?.state ?? {};
}

function projectAnchor(): void {
  const state = useItemsStore.getState();
  const { selectedItem, items, detail } = state;
  if (selectedItem === null) {
    usePreviewStore.getState().anchorCleared();
    return;
  }
  const item = items.find((candidate) => itemKey(candidate) === selectedItem);
  if (!item) return;
  const payload = {
    hash: item.hash,
    pathId: item.hash === null ? item.pathId : null,
  };
  const preview = usePreviewStore.getState();
  if (preview.follow && preview.placement === null) {
    void preview.open(payload, detail, appWindowState());
  } else {
    preview.anchorChanged(payload, detail);
  }
}

/** Install once for the lifetime of the main webview. */
export function installItemWorkflow(): void {
  if (installed) return;
  installed = true;
  useItemsStore.subscribe((state, previous) => {
    const patch: Record<string, unknown> = {};
    if (state.sortOrders !== previous.sortOrders) patch.sortOrders = state.sortOrders;
    if (state.selected !== previous.selected) patch.lastSection = state.selected;
    if (state.selectedItem !== previous.selectedItem) patch.lastItem = state.selectedItem;
    if (
      state.selected !== previous.selected ||
      state.selectedItem !== previous.selectedItem ||
      state.sortOrders !== previous.sortOrders
    ) {
      patch.lastItemContext = state.currentContext;
    }
    if (Object.keys(patch).length > 0) {
      void useAppStore.getState().patchState(patch);
    }
    const selectedEnteredWindow =
      state.selectedItem !== null &&
      state.items !== previous.items &&
      !previous.items.some((item) => itemKey(item) === state.selectedItem) &&
      state.items.some((item) => itemKey(item) === state.selectedItem);
    if (state.selectedItem !== previous.selectedItem || selectedEnteredWindow) {
      projectAnchor();
    } else if (state.detail !== previous.detail && state.detail !== null) {
      const item =
        state.selectedItem === null
          ? undefined
          : state.items.find((candidate) => itemKey(candidate) === state.selectedItem);
      if (item) {
        usePreviewStore.getState().detailLoaded(
          { hash: item.hash, pathId: item.hash === null ? item.pathId : null },
          state.detail,
        );
      }
    } else if (state.detail !== previous.detail && state.detail === null) {
      projectAnchor();
    }
  });
}

/** Deletes the current grid selection, preserving file-manager recovery. */
export async function deleteSelectedItems(permanent: boolean): Promise<void> {
  const { selectedItem, selectedKeys } = useItemsStore.getState();
  const keys =
    selectedKeys.size > 0
      ? selectedKeys
      : selectedItem !== null
        ? new Set([selectedItem])
        : new Set<string>();
  await deleteItems(keys, permanent);
}

/** Deletes an explicit logical-item set and refreshes every affected owner. */
export async function deleteItems(
  keys: Set<string>,
  permanent: boolean,
): Promise<void> {
  if (keys.size === 0) return;
  useItemsStore.setState({ message: null });
  try {
    const positions = useItemsStore.getState().selectedPositions;
    const orderedKeys = [...keys].sort(
      (left, right) =>
        (positions.get(left) ?? Number.MAX_SAFE_INTEGER) -
        (positions.get(right) ?? Number.MAX_SAFE_INTEGER),
    );
    const outcome = await invoke<DeleteBatchOutcome>("delete_items", {
      items: orderedKeys.map((key) => {
        const identity = identityFromKey(key);
        return {
          hash: identity.hash,
          pathId: identity.hash === null ? identity.pathId : null,
        };
      }),
      permanent,
    });
    if (outcome.error !== null) {
      useItemsStore.setState({ message: outcome.error });
      await useIssuesStore.getState().load();
    } else if (outcome.failedFiles > 0) {
      useItemsStore.setState({
        message: `${outcome.failedFiles} file${outcome.failedFiles === 1 ? "" : "s"} could not be deleted — see Issues.`,
      });
      await useIssuesStore.getState().load();
    }
    // The item store reconciles selection against the prior displayed order:
    // surviving selected members remain selected, then next/previous recovery
    // applies. A second hand-written recovery here used to erase that result.
    await useItemsStore.getState().refresh();
    await useSectionsStore.getState().loadCounts();
  } catch (error) {
    log.error("delete failed", toErrorFields(error));
    recordActionFailure("delete-start-failed", "The delete operation could not start.", error);
    useItemsStore.setState({
      message: error instanceof Error ? error.message : String(error),
    });
    // A structural error can arrive after earlier logical units committed.
    // Re-read every durable owner instead of leaving removed rows projected.
    await useItemsStore.getState().refresh();
    await useSectionsStore.getState().loadCounts();
    await useIssuesStore.getState().load();
  }
}

/** Re-stats only directories represented by the open section. */
export async function rescanCurrentSection(): Promise<void> {
  const selected = useItemsStore.getState().selected;
  if (!selected) return;
  try {
    await invoke<number>("rescan_section", {
      kind: selected.kind,
      month: selected.month,
    });
    await useItemsStore.getState().refresh();
    await useSectionsStore.getState().loadCounts();
  } catch (error) {
    log.error("section rescan failed", toErrorFields(error));
    const message = error instanceof Error ? error.message : String(error);
    if (!message.includes("scan cancelled")) {
      useItemsStore.setState({ message });
      recordActionFailure("section-refresh-failed", "Couldn’t refresh this section.", error);
      await useIssuesStore.getState().load();
    }
  }
}
