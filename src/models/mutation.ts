import { formatBytes } from "./items";

export type MutationKind =
  | "delete"
  | "destination-copy"
  | "destination-move"
  | "trash-empty";
export type MutationPhase =
  | "planning"
  | "deleting"
  | "delivering"
  | "emptying"
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
  currentFileBytesDone: number | null;
  currentFileBytesTotal: number | null;
  nextPhase: MutationPhase | null;
}

export interface MutationResultSummary {
  itemsCompleted: number;
  itemsPartial: number;
  itemsUnstarted: number;
  filesCompleted: number;
  filesFailed: number;
  filesUnstarted: number;
  error: string | null;
}

export interface MutationResult {
  operationId: number;
  kind: MutationKind;
  cancelled: boolean;
  summary: MutationResultSummary;
}

function actionName(kind: MutationKind): string {
  if (kind === "delete") return "Deletion";
  if (kind === "destination-copy") return "Copy";
  if (kind === "destination-move") return "Move";
  return "Trash emptying";
}

export function mutationResultLine(result: MutationResult): string {
  const { summary } = result;
  const parts = [
    `${summary.itemsCompleted.toLocaleString()} completed`,
  ];
  if (summary.itemsPartial > 0) {
    parts.push(`${summary.itemsPartial.toLocaleString()} partially processed`);
  }
  if (
    summary.filesCompleted > 0 &&
    (result.cancelled ||
      summary.error !== null ||
      summary.filesFailed > 0 ||
      summary.itemsPartial > 0 ||
      summary.itemsUnstarted > 0 ||
      summary.filesUnstarted > 0)
  ) {
    parts.push(`${summary.filesCompleted.toLocaleString()} file steps completed`);
  }
  if (summary.filesFailed > 0) {
    parts.push(`${summary.filesFailed.toLocaleString()} failed`);
  }
  if (summary.itemsUnstarted > 0) {
    parts.push(`${summary.itemsUnstarted.toLocaleString()} unstarted`);
  }
  if (summary.filesUnstarted > 0 && summary.itemsUnstarted === 0) {
    parts.push(`${summary.filesUnstarted.toLocaleString()} file steps unstarted`);
  }
  if (summary.error !== null) parts.push(summary.error);
  const state = summary.error !== null
    ? "stopped"
    : result.cancelled
      ? "cancelled"
      : summary.filesFailed > 0
        ? "finished with failures"
        : "complete";
  return `${actionName(result.kind)} ${state} — ${parts.join(" · ")}`;
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
        : progress.kind === "destination-move"
          ? "move"
          : "trash emptying";
  if (cancelling) return "Cancelling after current file…";
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
        : progress.kind === "destination-move"
          ? "Moving"
          : "Emptying Trash";
  const current =
    progress.currentFileBytesDone !== null &&
    progress.currentFileBytesTotal !== null &&
    progress.currentFileBytesTotal > 0
      ? ` · current ${Math.min(100, Math.floor((progress.currentFileBytesDone / progress.currentFileBytesTotal) * 100))}%`
      : "";
  return `${verb} — ${progress.itemsDone.toLocaleString()}/${progress.itemsTotal.toLocaleString()} items · ${progress.filesDone.toLocaleString()}/${progress.filesTotal.toLocaleString()} files · ${formatBytes(progress.bytesDone)}/${formatBytes(progress.bytesTotal)}${current}${progress.failures > 0 ? ` · ${progress.failures.toLocaleString()} failed` : ""}`;
}
