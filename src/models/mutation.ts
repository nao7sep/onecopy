import { formatBytes } from "./items";

export type MutationKind = "delete";
export type MutationPhase =
  | "planning"
  | "deleting"
  | "complete";

export interface MutationProgress {
  operationId: number;
  kind: MutationKind;
  phase: MutationPhase;
  itemsDone: number;
  itemsTotal: number;
  filesDone: number;
  filesTotal: number;
  bytesDone: number;
  bytesTotal: number;
  failures: number;
  nextPhase: MutationPhase | null;
}

export function mutationProgressLine(
  progress: MutationProgress,
  cancelling: boolean,
): string {
  const action = "deletion";
  if (cancelling) return `Stopping ${action}…`;
  if (progress.phase === "planning") {
    return `Planning ${action} — ${progress.itemsDone.toLocaleString()}/${progress.itemsTotal.toLocaleString()} items`;
  }
  if (progress.phase === "complete") {
    return "Deletion complete";
  }
  return `Deleting — ${progress.itemsDone.toLocaleString()}/${progress.itemsTotal.toLocaleString()} items · ${progress.filesDone.toLocaleString()}/${progress.filesTotal.toLocaleString()} files · ${formatBytes(progress.bytesDone)}/${formatBytes(progress.bytesTotal)}${progress.failures > 0 ? ` · ${progress.failures.toLocaleString()} failed` : ""}`;
}
