import { subscribeDisplayZone } from "../repositories/display-zone";
import { useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";

export function installDisplayZoneReconciliation(): () => void {
  return subscribeDisplayZone(() => {
    // Ordinary read reconciliation preserves surviving selection by identity
    // and recovers within the current section if its anchor changed month.
    // It does not recheck sources, reset failures, or re-resolve metadata.
    void useSectionsStore.getState().loadCounts();
    void useItemsStore.getState().refresh();
  });
}
