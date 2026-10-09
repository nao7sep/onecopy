import type { MessageKey } from "../i18n/catalogues";
import type { Translator } from "../i18n/translate";
import { formatBytes } from "./items";

export type MutationKind =
  | "delete"
  | "destination-copy"
  | "destination-move"
  | "trash-empty"
  | "restore";
export type MutationPhase =
  | "waiting"
  | "planning"
  | "deleting"
  | "delivering"
  | "emptying"
  | "restoring"
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
  /** Files whose rename or removal was given up on while their drive was not
   * responding: each may or may not have happened. */
  filesUnknown: number;
  filesUnstarted: number;
  /** Restore only: files left in Deleted files because their original path
   * already holds the same bytes. */
  filesAlreadyPresent?: number;
  trashAvailable: boolean;
  error: string | null;
}

export interface MutationResult {
  operationId: number;
  kind: MutationKind;
  cancelled: boolean;
  summary: MutationResultSummary;
}

type Outcome = "complete" | "cancelled" | "failures" | "stopped";

/** One headline per operation and outcome. A headline is never assembled from
 * an action word and a state word: which of the two leads, and how each is
 * inflected, differs by language. */
const OUTCOME_HEADLINES: Record<MutationKind, Record<Outcome, MessageKey>> = {
  delete: {
    complete: "mutation.deleteComplete",
    cancelled: "mutation.deleteCancelled",
    failures: "mutation.deleteWithFailures",
    stopped: "mutation.deleteStopped",
  },
  "destination-copy": {
    complete: "mutation.copyComplete",
    cancelled: "mutation.copyCancelled",
    failures: "mutation.copyWithFailures",
    stopped: "mutation.copyStopped",
  },
  "destination-move": {
    complete: "mutation.moveComplete",
    cancelled: "mutation.moveCancelled",
    failures: "mutation.moveWithFailures",
    stopped: "mutation.moveStopped",
  },
  "trash-empty": {
    complete: "mutation.emptyComplete",
    cancelled: "mutation.emptyCancelled",
    failures: "mutation.emptyWithFailures",
    stopped: "mutation.emptyStopped",
  },
  restore: {
    complete: "mutation.restoreComplete",
    cancelled: "mutation.restoreCancelled",
    failures: "mutation.restoreWithFailures",
    stopped: "mutation.restoreStopped",
  },
};

const PLANNING_HEADLINES: Record<MutationKind, MessageKey> = {
  delete: "mutation.planningDelete",
  "destination-copy": "mutation.planningCopy",
  "destination-move": "mutation.planningMove",
  "trash-empty": "mutation.planningEmpty",
  restore: "mutation.planningRestore",
};

const RUNNING_HEADLINES: Record<MutationKind, MessageKey> = {
  delete: "mutation.deleting",
  "destination-copy": "mutation.copying",
  "destination-move": "mutation.moving",
  "trash-empty": "mutation.emptyingTrash",
  restore: "mutation.restoring",
};

function outcome(result: MutationResult): Outcome {
  if (result.summary.error !== null) return "stopped";
  if (result.cancelled) return "cancelled";
  return result.summary.filesFailed > 0 || result.summary.filesUnknown > 0
    ? "failures"
    : "complete";
}

/** Whole percent of the file being handled right now, or null when the backend
 * reports no measurable current file. */
function currentFilePercent(progress: MutationProgress): number | null {
  const { currentFileBytesDone: done, currentFileBytesTotal: total } = progress;
  if (done === null || total === null || total <= 0) return null;
  return Math.min(100, Math.floor((done / total) * 100));
}

/** The receipt's words. The facts are a variable join of independently
 * translated counts — which of them appear depends on the outcome — so this
 * composes them with the translator instead of returning one descriptor. */
/** A restore's receipt: each selected file is its own unit, so the facts are
 * files restored, already at their original location, failed, of unknown
 * outcome and unstarted. */
function restoreResultLine(result: MutationResult, t: Translator["t"]): string {
  const { summary } = result;
  const facts = [t("mutation.factRestored", { count: summary.filesCompleted })];
  const already = summary.filesAlreadyPresent ?? 0;
  if (already > 0) facts.push(t("mutation.factAlreadyThere", { count: already }));
  if (summary.filesFailed > 0) facts.push(t("trash.failedCount", { count: summary.filesFailed }));
  if (summary.filesUnknown > 0) {
    facts.push(t("mutation.factOutcomeUnknown", { count: summary.filesUnknown }));
  }
  if (summary.filesUnstarted > 0) {
    facts.push(t("mutation.factFileStepsUnstarted", { count: summary.filesUnstarted }));
  }
  if (summary.error !== null) facts.push(t("mutation.factStopped"));
  return t("mutation.line", {
    headline: t(OUTCOME_HEADLINES.restore[outcome(result)]),
    facts: facts.join(" · "),
  });
}

export function mutationResultLine(
  result: MutationResult,
  t: Translator["t"],
): string {
  if (result.kind === "restore") return restoreResultLine(result, t);
  const { summary } = result;
  const facts = [t("mutation.factCompleted", { count: summary.itemsCompleted })];
  if (summary.itemsPartial > 0) {
    facts.push(t("mutation.factPartial", { count: summary.itemsPartial }));
  }
  if (
    summary.filesCompleted > 0 &&
    (result.cancelled ||
      summary.error !== null ||
      summary.filesFailed > 0 ||
      summary.filesUnknown > 0 ||
      summary.itemsPartial > 0 ||
      summary.itemsUnstarted > 0 ||
      summary.filesUnstarted > 0)
  ) {
    facts.push(t("mutation.factFileStepsCompleted", { count: summary.filesCompleted }));
  }
  if (summary.filesFailed > 0) {
    facts.push(t("trash.failedCount", { count: summary.filesFailed }));
  }
  if (summary.filesUnknown > 0) {
    facts.push(t("mutation.factOutcomeUnknown", { count: summary.filesUnknown }));
  }
  if (summary.itemsUnstarted > 0) {
    facts.push(t("mutation.factUnstarted", { count: summary.itemsUnstarted }));
  }
  if (summary.filesUnstarted > 0 && summary.itemsUnstarted === 0) {
    facts.push(t("mutation.factFileStepsUnstarted", { count: summary.filesUnstarted }));
  }
  if (summary.error !== null) {
    facts.push(t("mutation.factStopped"));
  }
  // Where deleted files went. The core reports Deleted files available only
  // for a recoverable delete that moved files there, so a delete that
  // removed files without it was permanent.
  if (result.kind === "delete" && summary.filesCompleted > 0) {
    facts.push(t(summary.trashAvailable ? "mutation.factRecoverable" : "mutation.factDeletedPermanently"));
  }
  return t("mutation.line", {
    headline: t(OUTCOME_HEADLINES[result.kind][outcome(result)]),
    facts: facts.join(" · "),
  });
}

export function mutationProgressLine(
  progress: MutationProgress,
  cancelling: boolean,
  t: Translator["t"],
  number: Translator["number"],
): string {
  if (cancelling) return t("mutation.cancellingAfterFile");
  // Background work is reaching its safe point; nothing is planned yet.
  if (progress.phase === "waiting") return t("mutation.waitingForBackgroundWork");
  if (progress.phase === "complete") {
    return t(OUTCOME_HEADLINES[progress.kind].complete);
  }
  // `count` drives the plural form of each fact; `done`/`total` are the numbers
  // the sentence shows. A restore counts files only: each is its own unit and
  // no bytes are copied.
  const facts =
    progress.kind === "restore"
      ? [
          t("mutation.factFiles", {
            count: progress.filesTotal,
            done: progress.filesDone,
            total: progress.filesTotal,
          }),
        ]
      : [
          t("mutation.factItems", {
            count: progress.itemsTotal,
            done: progress.itemsDone,
            total: progress.itemsTotal,
          }),
        ];
  if (progress.phase !== "planning" && progress.kind !== "restore") {
    facts.push(
      t("mutation.factFiles", {
        count: progress.filesTotal,
        done: progress.filesDone,
        total: progress.filesTotal,
      }),
      `${formatBytes(progress.bytesDone, number)}/${formatBytes(progress.bytesTotal, number)}`,
    );
  }
  const percent = currentFilePercent(progress);
  if (percent !== null) facts.push(t("mutation.factCurrent", { percent }));
  if (progress.phase !== "planning" && progress.failures > 0) {
    facts.push(t("trash.failedCount", { count: progress.failures }));
  }
  return t("mutation.line", {
    headline: t(
      progress.phase === "planning"
        ? PLANNING_HEADLINES[progress.kind]
        : RUNNING_HEADLINES[progress.kind],
    ),
    facts: facts.join(" · "),
  });
}
