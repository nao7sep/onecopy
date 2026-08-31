// Event adapter for the shared ephemeral item-mutation runtime. Installation
// is idempotent; domain workflows still own refreshes, selection, and results.

import { listen } from "@tauri-apps/api/event";
import type {
  MutationKind,
  MutationProgress,
  MutationResultSummary,
} from "../models/mutation";
import { log, toErrorFields } from "../repositories";
import { useMutationStore } from "../state/mutation-store";
import { useItemsStore } from "../state/items-store";
import { recordInterfaceFailure } from "../utils/failureSurface";

let installation: Promise<void> | null = null;

async function install(): Promise<void> {
  try {
    await listen<MutationProgress>("mutation://progress", (event) => {
      const current = useMutationStore.getState();
      const sameOperation = current.progress?.operationId === event.payload.operationId;
      useMutationStore.setState({
        progress: event.payload,
        cancelling: sameOperation ? current.cancelling : false,
      });
    });
    await listen<{
      progress: MutationProgress;
      cancelled: boolean;
      summary: MutationResultSummary | null;
    }>(
      "mutation://done",
      (event) => {
        if (
          useMutationStore.getState().progress?.operationId ===
          event.payload.progress.operationId
        ) {
          useMutationStore.setState({
            progress: null,
            cancelling: false,
            ...(event.payload.summary === null
              ? {}
              : {
                  result: {
                    operationId: event.payload.progress.operationId,
                    kind: event.payload.progress.kind,
                    cancelled: event.payload.cancelled,
                    summary: event.payload.summary,
                  },
                }),
          });
        }
      },
    );
    await listen<{
      operationId: number;
      kind: MutationKind;
      error: string;
      summary: MutationResultSummary;
    }>(
      "mutation://error",
      (event) => {
        if (useMutationStore.getState().progress?.operationId === event.payload.operationId) {
          useMutationStore.setState({
            progress: null,
            cancelling: false,
            result: {
              operationId: event.payload.operationId,
              kind: event.payload.kind,
              cancelled: false,
              summary: event.payload.summary,
            },
          });
        }
      },
    );
    await listen("app://exit-quiescing", () => {
      useMutationStore.setState({ exiting: true, cancelling: true });
    });
  } catch (error) {
    log.warn("file operation event wiring failed", toErrorFields(error));
    const message = error instanceof Error ? error.message : String(error);
    recordInterfaceFailure(message);
    useMutationStore.setState({ progress: null, cancelling: false });
    useItemsStore.setState({
      message: "Live file-operation status is unavailable. Restart OneCopy before changing more files.",
    });
  }
}

export function installMutationEventWiring(): Promise<void> {
  installation ??= install();
  return installation;
}
