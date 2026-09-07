// Event adapter for the shared ephemeral item-mutation runtime. Installation
// is idempotent; domain workflows still own refreshes, selection, and results.

import {
  mutationResultLine,
  type MutationResult,
  type MutationKind,
  type MutationProgress,
  type MutationResultSummary,
} from "../models/mutation";
import { log, toErrorFields } from "../repositories";
import { useMutationStore } from "../state/mutation-store";
import { useItemsStore } from "../state/items-store";
import { recordRecentNotification } from "../state/notifications-store";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { createEventInstaller } from "../utils/eventInstallation";

const install = createEventInstaller(
  async (listeners) => {
    const recordFailedResult = (result: MutationResult) => {
      if (result.summary.error === null && result.summary.filesFailed === 0) return;
      void recordRecentNotification({
        kind: `${result.kind}-failed`,
        level: result.summary.error === null ? "warning" : "error",
        presentation: "persistent",
        message: mutationResultLine(result),
      }).catch((error) => {
        log.error("file-operation result recording failed", toErrorFields(error));
        recordInterfaceFailure("OneCopy could not save the file-operation result.");
      });
    };
    await listeners.listen<MutationProgress>("mutation://progress", (event) => {
      const current = useMutationStore.getState();
      const sameOperation = current.progress?.operationId === event.payload.operationId;
      useMutationStore.setState({
        progress: event.payload,
        cancelling: sameOperation ? current.cancelling : false,
      });
    });
    await listeners.listen<{
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
          const result = event.payload.summary === null
            ? null
            : {
                operationId: event.payload.progress.operationId,
                kind: event.payload.progress.kind,
                cancelled: event.payload.cancelled,
                summary: event.payload.summary,
              } satisfies MutationResult;
          useMutationStore.setState({
            progress: null,
            cancelling: false,
            ...(result === null
              ? {}
              : { result }),
          });
          if (result !== null) recordFailedResult(result);
        }
      },
    );
    await listeners.listen<{
      operationId: number;
      kind: MutationKind;
      error: string;
      summary: MutationResultSummary;
    }>(
      "mutation://error",
      (event) => {
        if (useMutationStore.getState().progress?.operationId === event.payload.operationId) {
          const result = {
            operationId: event.payload.operationId,
            kind: event.payload.kind,
            cancelled: false,
            summary: event.payload.summary,
          } satisfies MutationResult;
          useMutationStore.setState({
            progress: null,
            cancelling: false,
            result,
          });
          recordFailedResult(result);
        }
      },
    );
    await listeners.listen("app://exit-quiescing", () => {
      useMutationStore.setState({ exiting: true, cancelling: true });
    });
  },
  (error) => {
    log.warn("file operation event wiring failed", toErrorFields(error));
    recordInterfaceFailure(
      "Live file-operation status is unavailable. Restart OneCopy before changing more files.",
    );
    useMutationStore.setState({ progress: null, cancelling: false });
    useItemsStore.setState({
      message: "Live file-operation status is unavailable. Restart OneCopy before changing more files.",
    });
  },
);

export function installMutationEventWiring(): Promise<void> {
  return install();
}
