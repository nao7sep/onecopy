import { formatBytes } from "./items";

export type MutationKind = "delete" | "destination-copy" | "destination-move";
export type MutationPhase = "planning" | "deleting" | "delivering" | "complete";

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
  currentFileBytesDone: number | null;
  currentFileBytesTotal: number | null;
  nextPhase: MutationPhase | null;
}

export function mutationProgressLine(
  progress: MutationProgress,
  cancelling: boolean,
): string {
  const action =
    progress.kind === "delete"
      ? "deletion"
      : progress.kind === "destination-copy"
        ? "copy"
        : "move";
  if (cancelling) return `Stopping ${action}…`;
  if (progress.phase === "planning") {
    const current =
      progress.currentFileBytesDone !== null &&
      progress.currentFileBytesTotal !== null &&
      progress.currentFileBytesTotal > 0
        ? ` · current ${Math.min(100, Math.floor((progress.currentFileBytesDone / progress.currentFileBytesTotal) * 100))}%`
        : "";
    return `Planning ${action} — ${progress.itemsDone.toLocaleString()}/${progress.itemsTotal.toLocaleString()} items${current}`;
  }
  if (progress.phase === "complete") {
    return `${action[0].toUpperCase()}${action.slice(1)} complete`;
  }
  const verb =
    progress.kind === "delete"
      ? "Deleting"
      : progress.kind === "destination-copy"
        ? "Copying"
        : "Moving";
  const current =
    progress.currentFileBytesDone !== null &&
    progress.currentFileBytesTotal !== null &&
    progress.currentFileBytesTotal > 0
      ? ` · current ${Math.min(100, Math.floor((progress.currentFileBytesDone / progress.currentFileBytesTotal) * 100))}%`
      : "";
  return `${verb} — ${progress.itemsDone.toLocaleString()}/${progress.itemsTotal.toLocaleString()} items · ${progress.filesDone.toLocaleString()}/${progress.filesTotal.toLocaleString()} files · ${formatBytes(progress.bytesDone)}/${formatBytes(progress.bytesTotal)}${current}${progress.failures > 0 ? ` · ${progress.failures.toLocaleString()} failed` : ""}`;
}
