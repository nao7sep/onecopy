import { invoke } from "@tauri-apps/api/core";
import { identityFromKey } from "../models/items";
import { viewerMainIndex } from "../models/viewerSession";
import { resolveWorkAttention, type ViewportAttention } from "../models/workAttention";
import { useItemsStore } from "../state/items-store";
import { comparisonChunks, useComparisonStore } from "../state/comparison-store";
import { useQuickViewStore } from "../state/quick-view-store";
import { log, toErrorFields } from "../repositories";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { latestActivityOperationId, newActivityOperationId, recordActivity } from "../repositories/activity";

let viewport: ViewportAttention | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let installed = false;
let last = "";
let generation = 0;

export function setWorkViewport(next: ViewportAttention): void {
  viewport = next;
  schedule();
}

function schedule(): void {
  if (!installed || timer !== null) return;
  timer = setTimeout(() => { timer = null; publish(); }, 100);
}

function publish(): void {
  const items = useItemsStore.getState();
  const section = items.selected;
  const currentViewport = viewport?.sectionKey === `${section?.kind}:${section?.month}` ? viewport : null;
  const comparison = useComparisonStore.getState();
  const viewer = useQuickViewStore.getState().session;
  const attention = resolveWorkAttention({
    selectedHash: items.selectedItem === null ? null : identityFromKey(items.selectedItem).hash,
    visibleHashes: currentViewport?.visibleHashes ?? [],
    nearbyHashes: currentViewport?.nearbyHashes ?? [],
    sectionKind: section?.kind ?? null,
    sectionMonth: section?.month ?? null,
    sectionSort: items.currentSort(),
    sectionAnchor: currentViewport?.anchor ?? 0,
    sectionTotal: items.totalItems,
  }, comparison.open ? {
    selected: comparison.anchor,
    visible: comparisonChunks(comparison).flat().map(({ member }) => member.hash),
  } : null, viewer === null ? null : {
    hash: viewer.member.hash,
    sectionIndex: viewerMainIndex(viewer, {
      section, sort: items.currentSort(), revision: items.reconciliationId,
      loading: items.loading, positions: items.itemPositions,
    }),
  });
  const signature = JSON.stringify(attention);
  if (signature === last) return;
  last = signature;
  generation = Math.max(Date.now() * 1000, generation + 1);
  const operationId = newActivityOperationId("priority");
  recordActivity({
    kind: "changed", owner: "priority", operationId,
    causeId: latestActivityOperationId("selection") ?? latestActivityOperationId("section"),
    current: "running", reason: "viewportChange",
    lane: attention.sectionKind ?? undefined, itemCount: attention.visibleHashes.length,
  });
  void invoke("prioritize_derived_work", { ...attention, generation }).then(() => {
    recordActivity({ kind: "completed", owner: "priority", operationId, previous: "running", current: "succeeded", reason: "completion" });
  }).catch((error) => {
    if (last === signature) last = "";
    log.warn("work attention update failed", toErrorFields(error));
    recordInterfaceFailure("Background work could not follow the current view. Try changing the selection.");
  });
}

/** Only Main installs subscriptions; auxiliary windows report through their session owners. */
export function installWorkAttention(): void {
  if (installed) return;
  installed = true;
  useItemsStore.subscribe(schedule);
  useComparisonStore.subscribe(schedule);
  useQuickViewStore.subscribe(schedule);
  schedule();
}
