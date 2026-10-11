import { emit } from "@tauri-apps/api/event";
import { useAppShellStore } from "../state/app-shell-store";
import { createEventInstaller } from "../utils/eventInstallation";
import { closeFullscreenView } from "./fullscreen-view";
import { focusWhileActive, log, toErrorFields } from "../repositories";
import { reportActionFailure } from "../state/notifications-store";
import { message } from "../i18n/translate";

export function requestBackgroundWork(): Promise<void> {
  return emit("viewing://open-background-work");
}

/** Main owns management. A viewing window only requests this existing surface. */
export async function openBackgroundWorkFromViewing(): Promise<void> {
  await closeFullscreenView({ restoreFocus: false });
  useAppShellStore.getState().openUtility("backgroundWork");
  await focusWhileActive("main");
}

export const installAuxiliaryUtilityWiring = createEventInstaller(async (listeners) => {
  await listeners.listen("viewing://open-background-work", () => {
    void openBackgroundWorkFromViewing().catch((error) => {
      log.error("viewing window could not open Background work", toErrorFields(error));
      reportActionFailure("viewing-background-work-failed", message("preview.commandFailed"), error);
    });
  });
}, () => undefined, { propagateFailure: true });
