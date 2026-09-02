// Preview application journeys. The preview store owns its surface and native
// window mechanics; this edge supplies persisted app state and the current
// item selection, and persists public Preview choices one way.

import { retainStatePatch, useAppStore } from "../state/app-store";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { reportWindowCall } from "../repositories";
import { useItemsStore } from "../state/items-store";
import { itemKey } from "../models/items";
import {
  type PlacementPreference,
  type PreviewPayload,
  usePreviewStore,
} from "../state/preview-store";

let persistenceInstalled = false;
let commandInstallation: Promise<void> | null = null;

interface PreviewKeyMessage {
  key: string;
  code?: string;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}

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
      retainStatePatch(patch);
    }
  });
}

/** The separate Preview is a follower, so it forwards library commands to
 * the Main grid instead of maintaining a second navigation implementation. */
export function installPreviewCommandWiring(): Promise<void> {
  commandInstallation ??= listen<PreviewKeyMessage>("preview://key", async (event) => {
    const message = event.payload;
    const area = document.getElementById("main-item-area");
    if (area === null) return;
    const needsConfirmation =
      (message.key === "Delete" || message.key === "Backspace") &&
      (message.shiftKey === true ||
        useAppStore.getState().appData?.config?.confirmTrashDelete === true);
    if (needsConfirmation) {
      await getCurrentWindow().setFocus().catch(reportWindowCall("main setFocus"));
    }
    area.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: message.key,
        code: message.code,
        shiftKey: message.shiftKey,
        metaKey: message.metaKey,
        ctrlKey: message.ctrlKey,
        altKey: message.altKey,
        bubbles: true,
        cancelable: true,
      }),
    );
  })
    .then(() => undefined)
    .catch((error) => {
      commandInstallation = null;
      throw error;
    });
  return commandInstallation;
}

export async function openPreview(
  payload: PreviewPayload,
  detail: ReturnType<typeof useItemsStore.getState>["detail"],
): Promise<void> {
  await usePreviewStore.getState().open(payload, detail, windowState());
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
  const state = useItemsStore.getState();
  const { items, selectedItem, detail } = state;
  const item =
    selectedItem === null
      ? undefined
      : items.find((item) => itemKey(item) === selectedItem);
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
