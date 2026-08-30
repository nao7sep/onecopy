// Item journeys at the application edge. The item store owns selection and
// request sequencing; this module persists its public choices, projects the
// anchor into Preview, and coordinates mutations with Issues and counts.

import { invoke } from "@tauri-apps/api/core";
import { sortItems } from "../models/items";
import { anchorContext } from "../models/mainSelection";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useIssuesStore } from "../state/issues-store";
import { itemKey, useItemsStore } from "../state/items-store";
import { usePreviewStore } from "../state/preview-store";
import { useSectionsStore } from "../state/sections-store";

interface ItemIdentity {
  hash: string | null;
  pathId: number | null;
}

interface DeleteItemResult {
  item: ItemIdentity;
  failedFiles: number;
}

interface DeleteBatchOutcome {
  cancelled: boolean;
  error: string | null;
  items: DeleteItemResult[];
  failedFiles: number;
}

function identityKey(item: ItemIdentity): string {
  return item.hash ?? `path-${item.pathId}`;
}

let installed = false;

function appWindowState(): Record<string, unknown> {
  return useAppStore.getState().appData?.state ?? {};
}

function projectAnchor(): void {
  const { selectedItem, items, detail } = useItemsStore.getState();
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
    void preview.open(payload, detail, {}, appWindowState());
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
      const order = sortItems(state.items, state.currentSort()).map(itemKey);
      patch.lastItemContext = anchorContext(order, state.selectedItem);
    }
    if (Object.keys(patch).length > 0) {
      void useAppStore.getState().patchState(patch);
    }
    if (state.selectedItem !== previous.selectedItem) {
      projectAnchor();
    } else if (state.detail !== previous.detail && state.detail !== null) {
      const item = state.items.find(
        (candidate) => itemKey(candidate) === state.selectedItem,
      );
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
  const store = useItemsStore.getState();
  if (keys.size === 0) return;
  // Recovery follows the displayed order, not the backend's natural order.
  const shown = sortItems(store.items, store.currentSort());
  const anchorIndex =
    store.selectedItem !== null
      ? shown.findIndex((item) => itemKey(item) === store.selectedItem)
      : shown.findIndex((item) => keys.has(itemKey(item)));
  useItemsStore.setState({ message: null });
  try {
    const requested = shown.filter((candidate) => keys.has(itemKey(candidate)));
    const outcome = await invoke<DeleteBatchOutcome>("delete_items", {
      items: requested.map((item) => ({
        hash: item.hash,
        pathId: item.hash === null ? item.pathId : null,
      })),
      permanent,
    });
    const completed = new Set(
      outcome.items
        .filter((item) => item.failedFiles === 0)
        .map((item) => identityKey(item.item)),
    );
    const survivor =
      shown.slice(Math.max(anchorIndex, 0)).find((item) => !completed.has(itemKey(item))) ??
      [...shown.slice(0, Math.max(anchorIndex, 0))]
        .reverse()
        .find((item) => !completed.has(itemKey(item))) ??
      null;
    if (outcome.error !== null) {
      useItemsStore.setState({ message: outcome.error });
      await useIssuesStore.getState().load();
    } else if (outcome.failedFiles > 0) {
      useItemsStore.setState({
        message: `${outcome.failedFiles} file${outcome.failedFiles === 1 ? "" : "s"} could not be deleted — see Issues.`,
      });
      await useIssuesStore.getState().load();
    }
    // Rows vanish first; then the anchor lands beside the removed run.
    await useItemsStore.getState().refresh();
    useItemsStore.getState().selectItem(survivor ? itemKey(survivor) : null);
    await useSectionsStore.getState().loadCounts();
  } catch (error) {
    log.error("delete failed", toErrorFields(error));
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
      await useIssuesStore.getState().load();
    }
  }
}
