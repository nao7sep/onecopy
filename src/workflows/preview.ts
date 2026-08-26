// Preview application journeys. The preview store owns its surface and native
// window mechanics; this edge supplies persisted app state and the current
// item selection, and persists public Preview choices one way.

import { useAppStore } from "../state/app-store";
import { itemKey, useItemsStore } from "../state/items-store";
import {
  type PlacementPreference,
  type PreviewIntent,
  type PreviewPayload,
  usePreviewStore,
} from "../state/preview-store";

let persistenceInstalled = false;

function windowState(): Record<string, unknown> {
  return useAppStore.getState().appData?.state ?? {};
}

export function installPreviewPersistence(): void {
  if (persistenceInstalled) return;
  persistenceInstalled = true;
  usePreviewStore.subscribe((state, previous) => {
    const patch: Record<string, unknown> = {};
    if (state.follow !== previous.follow) patch.previewFollow = state.follow;
    if (state.placementPreference !== previous.placementPreference) {
      patch.previewPlacement = state.placementPreference;
    }
    if (Object.keys(patch).length > 0) {
      void useAppStore.getState().patchState(patch);
    }
  });
}

export async function openPreview(
  payload: PreviewPayload,
  detail: ReturnType<typeof useItemsStore.getState>["detail"],
  intent: PreviewIntent = {},
): Promise<void> {
  await usePreviewStore.getState().open(payload, detail, intent, windowState());
}

export function closePreview(): void {
  usePreviewStore.getState().close();
}

export async function togglePreview(): Promise<void> {
  const preview = usePreviewStore.getState();
  if (preview.follow) {
    preview.close();
    return;
  }
  const { items, selectedItem, detail } = useItemsStore.getState();
  const item = items.find((candidate) => itemKey(candidate) === selectedItem);
  if (!item) {
    // Arm follow without opening an empty surface. The first real anchor is
    // projected by the installed item workflow.
    usePreviewStore.setState({ follow: true });
    return;
  }
  await openPreview(
    { hash: item.hash, pathId: item.hash === null ? item.pathId : null },
    detail,
  );
}

export async function setPreviewPlacement(
  preference: PlacementPreference,
): Promise<void> {
  await usePreviewStore
    .getState()
    .setPlacementPreference(preference, windowState());
}
