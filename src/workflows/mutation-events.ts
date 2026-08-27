// Event adapter for the shared ephemeral item-mutation runtime. Installation
// is idempotent; domain workflows still own refreshes, selection, and results.

import { listen } from "@tauri-apps/api/event";
import type { MutationKind, MutationProgress } from "../models/mutation";
import { log, toErrorFields } from "../repositories";
import { useMutationStore } from "../state/mutation-store";

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
    await listen<{ progress: MutationProgress; cancelled: boolean }>(
      "mutation://done",
      (event) => {
        if (
          useMutationStore.getState().progress?.operationId ===
          event.payload.progress.operationId
        ) {
          useMutationStore.setState({ progress: null, cancelling: false });
        }
      },
    );
    await listen<{ operationId: number; kind: MutationKind; error: string }>(
      "mutation://error",
      (event) => {
        if (useMutationStore.getState().progress?.operationId === event.payload.operationId) {
          useMutationStore.setState({ progress: null, cancelling: false });
        }
      },
    );
  } catch (error) {
    log.warn("file operation event wiring failed", toErrorFields(error));
  }
}

export function installMutationEventWiring(): Promise<void> {
  installation ??= install();
  return installation;
}
