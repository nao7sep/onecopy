// Application-edge reactions for the Issues owner's restart-persistent views.
// Notification presentation and issue history deliberately remain separate
// stores; backend events are the boundary that keeps an open history current.

import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";
import { useIssuesStore } from "../state/issues-store";
import { recordInterfaceFailure } from "../utils/failureSurface";

let installation: Promise<void> | null = null;

function refreshRecentWhenOpen(): void {
  const issues = useIssuesStore.getState();
  if (issues.open) void issues.loadRecent();
}

async function install(): Promise<void> {
  const unlisten: Array<() => void> = [];
  try {
    unlisten.push(await listen("notification://published", refreshRecentWhenOpen));
    unlisten.push(await listen("notification://recorded", refreshRecentWhenOpen));
  } catch (error) {
    for (const stop of unlisten) stop();
    throw error;
  }
}

export function installIssuesEventWiring(): Promise<void> {
  installation ??= install().catch((error) => {
    installation = null;
    log.error("issues event wiring failed", toErrorFields(error));
    const reason = error instanceof Error ? error.message : String(error);
    recordInterfaceFailure(
      `Issue history will not update while it is open. Reopen it to refresh: ${reason}`,
    );
  });
  return installation;
}
