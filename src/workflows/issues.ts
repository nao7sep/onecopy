// Notification failures refresh both the current-run inbox and status count.

import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";
import { useIssuesStore } from "../state/issues-store";
import { message } from "../i18n/translate";
import { recordInterfaceFailure } from "../utils/failureSurface";

let installation: Promise<void> | null = null;

function refreshIssues(): void {
  void useIssuesStore.getState().load();
}

async function install(): Promise<void> {
  const unlisten: Array<() => void> = [];
  try {
    unlisten.push(await listen("notification://published", refreshIssues));
    unlisten.push(await listen("notification://recorded", refreshIssues));
  } catch (error) {
    for (const stop of unlisten) stop();
    throw error;
  }
}

export function installIssuesEventWiring(): Promise<void> {
  installation ??= install().catch((error) => {
    installation = null;
    log.error("issues event wiring failed", toErrorFields(error));
    recordInterfaceFailure(message("issues.liveUpdatesUnavailable"));
  });
  return installation;
}
