import type { MessageKey } from "../i18n/catalogues";
import type { Translator } from "../i18n/translate";
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
};

const PLANNING_HEADLINES: Record<MutationKind, MessageKey> = {
  delete: "mutation.planningDelete",
  "destination-copy": "mutation.planningCopy",
  "destination-move": "mutation.planningMove",
  "trash-empty": "mutation.planningEmpty",
};

const RUNNING_HEADLINES: Record<MutationKind, MessageKey> = {
  delete: "mutation.deleting",
  "destination-copy": "mutation.copying",
  "destination-move": "mutation.moving",
  "trash-empty": "mutation.emptyingTrash",
};

function outcome(result: MutationResult): Outcome {
  if (result.summary.error !== null) return "stopped";
  if (result.cancelled) return "cancelled";
  return result.summary.filesFailed > 0 ? "failures" : "complete";
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
export function mutationResultLine(
  result: MutationResult,
  t: Translator["t"],
): string {
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
      summary.itemsPartial > 0 ||
      summary.itemsUnstarted > 0 ||
      summary.filesUnstarted > 0)
  ) {
    facts.push(t("mutation.factFileStepsCompleted", { count: summary.filesCompleted }));
  }
  if (summary.filesFailed > 0) {
    facts.push(t("trash.failedCount", { count: summary.filesFailed }));
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
  if (progress.phase === "complete") {
    return t(OUTCOME_HEADLINES[progress.kind].complete);
  }
  // `count` drives the plural form of each fact; `done`/`total` are the numbers
  // the sentence shows.
  const facts = [
    t("mutation.factItems", {
      count: progress.itemsTotal,
      done: progress.itemsDone,
      total: progress.itemsTotal,
    }),
  ];
  if (progress.phase !== "planning") {
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
