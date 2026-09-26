// Item journeys at the application edge. The item store owns selection and
// request sequencing; this module persists its public choices, projects the
// anchor into Preview, and coordinates mutations with Issues and counts.

import { invoke } from "@tauri-apps/api/core";
import { identityFromKey, itemKey } from "../models/items";
import { log, toErrorFields } from "../repositories";
import { retainStatePatch, useAppStore } from "../state/app-store";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import { beginMainFeedback } from "../state/main-feedback-store";
import { usePreviewStore } from "../state/preview-store";
import { useSectionsStore } from "../state/sections-store";
import { message } from "../i18n/translate";
import { recordActionFailure, reportActionFailure } from "../state/notifications-store";

interface DeleteBatchOutcome {
  error: string | null;
  failedFiles: number;
}

type RescanSectionOutcome =
  | { status: "completed"; changed: number }
  | { status: "cancelled" };

let installed = false;

function appWindowState(): Record<string, unknown> {
  return useAppStore.getState().appData?.state ?? {};
}

function selectedCount(state: ReturnType<typeof useItemsStore.getState>): number {
  return state.selectedKeys.size > 0 ? state.selectedKeys.size : state.selectedItem !== null ? 1 : 0;
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
    selectedCount: selectedCount(state),
  };
  const preview = usePreviewStore.getState();
  // A failed open stays failed until the user acts on the offered recovery
  // (see `openFailed` on the preview store); otherwise every subsequent
  // anchor change would retry — and re-fail — the same broken open.
  if (preview.follow && preview.placement === null && !preview.openFailed) {
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
      retainStatePatch(patch);
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
          {
            hash: item.hash,
            pathId: item.hash === null ? item.pathId : null,
            selectedCount: selectedCount(state),
          },
          state.detail,
        );
      }
    } else if (state.detail !== previous.detail && state.detail === null) {
      projectAnchor();
    }
  });
}

/** Freezes the current grid selection, in section order, as the exact
 * logical-item set a deletion review shows and a confirmed deletion acts on.
 * Nothing re-reads the live selection after this capture: a refresh that
 * recovers the anchor to a neighbour must never retarget the deletion. */
export function captureDeleteSelection(): string[] {
  const { selectedItem, selectedKeys, selectedPositions } = useItemsStore.getState();
  const keys =
    selectedKeys.size > 0
      ? [...selectedKeys]
      : selectedItem !== null
        ? [selectedItem]
        : [];
  return keys.sort(
    (left, right) =>
      (selectedPositions.get(left) ?? Number.MAX_SAFE_INTEGER) -
      (selectedPositions.get(right) ?? Number.MAX_SAFE_INTEGER),
  );
}

/** Deletes an explicit, already-frozen ordered logical-item set and
 * refreshes every affected owner. */
export async function deleteItems(
  keys: readonly string[],
  permanent: boolean,
): Promise<void> {
  if (keys.length === 0) return;
  try {
    const outcome = await invoke<DeleteBatchOutcome>("delete_items", {
      items: keys.map((key) => {
        const identity = identityFromKey(key);
        return {
          hash: identity.hash,
          pathId: identity.hash === null ? identity.pathId : null,
        };
      }),
      permanent,
    });
    // The mutation runtime owns the complete operation receipt and its
    // dismissal. Do not shadow it with a second sticky item-store message.
    if (outcome.error !== null || outcome.failedFiles > 0) {
      await useIssuesStore.getState().load();
    }
    // The item store reconciles selection against the prior displayed order:
    // surviving selected members remain selected, then next/previous recovery
    // applies. A second hand-written recovery here used to erase that result.
    await useItemsStore.getState().refresh();
    await useSectionsStore.getState().loadCounts();
  } catch (error) {
    log.error("delete failed", toErrorFields(error));
    reportActionFailure(
      "delete-start-failed",
      message("item.deleteFailed"),
      error,
    );
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
  const feedback = beginMainFeedback("recheck");
  try {
    const outcome = await invoke<RescanSectionOutcome>("rescan_section", {
      kind: selected.kind,
      month: selected.month,
    });
    if (outcome.status === "cancelled") return;
    await useItemsStore.getState().refresh();
    await useSectionsStore.getState().loadCounts();
    feedback.finish();
  } catch (error) {
    log.error("section rescan failed", toErrorFields(error));
    feedback.finish({ tone: "danger", text: message("section.rescanFailed") });
    recordActionFailure("section-refresh-failed", message("section.rescanFailedNotice"), error);
    await useIssuesStore.getState().load();
  }
}
