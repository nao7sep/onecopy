import type { MessageKey } from "../i18n/catalogues";
import { reasonText } from "./workReasons";
import type { Translator } from "../i18n/translate";
import { formatLocalMinute } from "../utils/displayTime";
import type { ItemDetail, ItemWorkState, ItemWorkStates, SectionItem } from "./items";

export type PresentationTone = "muted" | "primary" | "warning" | "danger";

// Every value here is resolved words, not a descriptor: a badge is a variable
// join of independently translated facts, and each of these runs on the render
// that shows it, so a language change re-composes it.
export interface PresentationBadge {
  text: string;
  label: string;
  tone: PresentationTone;
}

export interface FaceRatingPresentation {
  stars: 1 | 2 | 3;
  label: string;
  tone: PresentationTone;
}

export interface SelectionMark {
  ordinal: number | null;
  label: string;
}

export interface WorkPresentationRow {
  id: keyof ItemWorkStates;
  label: string;
  value: string;
  tone: PresentationTone;
}

export interface ItemPresentation {
  selection: SelectionMark | null;
  status: PresentationBadge | null;
  relationships: PresentationBadge | null;
  analysis: PresentationBadge | FaceRatingPresentation | null;
}

const WORK_LABELS: Record<keyof ItemWorkStates, MessageKey> = {
  preview: "preview.label",
  snapshots: "wizard.videoSnapshots",
  similarity: "work.classSimilarity",
  faces: "wizard.faceScoring",
  transcripts: "activity.actionTranscript",
};

const WORK_SHORT_LABELS: Record<keyof ItemWorkStates, MessageKey> = {
  preview: "preview.label",
  snapshots: "metadata.snapshots",
  similarity: "item.workSimilarityShort",
  faces: "item.workFacesShort",
  transcripts: "transcript.title",
};

const WORK_ORDER = [
  "preview",
  "snapshots",
  "similarity",
  "faces",
  "transcripts",
] as const satisfies readonly (keyof ItemWorkStates)[];

/** What a class says when it has finished and found nothing to show. */
const READY_WITHOUT_VALUE: Partial<Record<keyof ItemWorkStates, MessageKey>> = {
  faces: "item.workNoFaceDetected",
  transcripts: "item.workNoSpeech",
  similarity: "item.workNoSimilarPhotos",
  snapshots: "item.workNoSnapshots",
};

const WORK_HELD: Record<"disabled" | "unavailable" | "blocked" | "waiting", MessageKey> = {
  disabled: "item.workOff",
  unavailable: "item.workUnavailable",
  blocked: "item.workBlocked",
  waiting: "activity.stateWaiting",
};

const SOURCE_KEYS: Record<string, MessageKey> = {
  metadata: "item.sourceMetadata",
  filename: "item.sourceFilename",
  filesystem: "item.sourceFilesystem",
};

export function takenPresentation(
  detail: ItemDetail,
  t: Translator["t"],
  dateTime: Translator["dateTime"],
): string {
  if (detail.dateState === "pending") return t("item.datePending");
  if (detail.resolvedUtcMs === null) return t("section.undated");
  const time = formatLocalMinute(detail.resolvedUtcMs, dateTime);
  const taken = detail.dateOnly ? t("item.takenDateOnly", { time }) : time;
  // The core reports where the date came from as a code; an unknown one shows
  // as recorded rather than as nothing.
  const source = SOURCE_KEYS[detail.resolvedSource ?? ""];
  if (detail.resolvedSource === null || detail.resolvedSource === "") return taken;
  return t("item.takenWithSource", {
    taken,
    source: source === undefined ? detail.resolvedSource : t(source),
  });
}

/** Wraps `headline` in the class's live progress, when the backend reports any.
 * The headline is a whole translated unit interpolated into the sentence, never
 * a fragment glued onto a number. */
function withProgress(
  headline: string,
  state: ItemWorkState,
  t: Translator["t"],
): string {
  if (state.done === null || state.total === null || state.total <= 0) return headline;
  return state.total === 100
    ? t("item.workProgressPercent", {
        headline,
        percent: Math.min(100, Math.round(state.done)),
      })
    : t("item.workProgressCount", {
        headline,
        done: state.done,
        total: state.total,
      });
}

export function workPresentationRows(
  states: ItemWorkStates,
  t: Translator["t"],
): WorkPresentationRow[] {
  return WORK_ORDER.flatMap((id) => {
    const state = states[id];
    if (state === null) return [];
    let value: string;
    let tone: PresentationTone = "muted";
    if (state.state === "ready") {
      value = state.hasValue
        ? t("item.workReady")
        : t(READY_WITHOUT_VALUE[id] ?? "item.workReady");
    } else if (state.state === "running") {
      value = withProgress(reasonText(state.reason, t) ?? t("item.workRunning"), state, t);
      tone = "primary";
    } else if (state.state === "failed") {
      value = reasonText(state.reason, t) ?? t("activity.stateFailed");
      tone = "danger";
    } else if (state.state === "pending") {
      value = t("work.queued");
    } else {
      value = reasonText(state.reason, t) ?? t(WORK_HELD[state.state]);
      tone = state.state === "disabled" ? "muted" : "warning";
    }
    return [{ id, label: t(WORK_LABELS[id]), value, tone }];
  });
}

function workStatus(
  states: ItemWorkStates,
  t: Translator["t"],
): PresentationBadge | null {
  const rows = WORK_ORDER.flatMap((id) => {
    const state = states[id];
    return state === null ? [] : [{ id, state }];
  });
  const select = (
    predicate: (id: keyof ItemWorkStates, state: ItemWorkState) => boolean,
  ) => rows.find(({ id, state }) => predicate(id, state));
  const chosen =
    select((_, state) => state.state === "failed") ??
    select((_, state) => state.state === "running") ??
    select((_, state) => state.state === "blocked" || state.state === "waiting") ??
    select(
      (id, state) =>
        (id === "preview" || id === "snapshots") &&
        (state.state === "unavailable" || state.state === "pending"),
    );
  if (chosen === undefined) return null;
  const { id, state } = chosen;
  const short = t(WORK_SHORT_LABELS[id]);
  const full = t(WORK_LABELS[id]);
  if (state.state === "failed") {
    return {
      text: t("item.workFailed", { name: short }),
      label: t("item.workFailed", { name: full }),
      tone: "danger",
    };
  }
  if (state.state === "running") {
    const hasProgress =
      state.done !== null && state.total !== null && state.total > 0;
    return {
      text: hasProgress
        ? withProgress(short, state, t)
        : t("work.classRunning", { name: short }),
      label: withProgress(t("item.workRunningLabel", { name: full }), state, t),
      tone: "primary",
    };
  }
  return {
    text:
      state.state === "pending"
        ? t("item.workQueuedShort", { name: short })
        : t("item.workWaitingShort", { name: short }),
    label: t("item.workLabelDetail", {
      name: full,
      detail:
        reasonText(state.reason, t) ??
        t(state.state === "pending" ? "work.queued" : "activity.stateWaiting"),
    }),
    tone: state.state === "pending" ? "muted" : "warning",
  };
}

/** The observed generated-face range is 0.558–0.669. Zero means no detected
 * face; positive scores are the best face's confidence weighted by happiness. */
export function faceStarRating(score: number | null): 0 | 1 | 2 | 3 {
  if (score === null || !Number.isFinite(score) || score <= 0) return 0;
  if (score < 0.58) return 1;
  if (score < 0.65) return 2;
  return 3;
}

export function itemPresentation(
  item: SectionItem,
  options: {
    similarCount: number;
    selectionOrdinal: number | null;
    selectedCount: number;
    showFaceStars: boolean;
  },
  t: Translator["t"],
): ItemPresentation {
  const selection =
    options.selectionOrdinal === null
      ? null
      : {
          ordinal: options.selectedCount > 1 ? options.selectionOrdinal : null,
          label:
            options.selectedCount > 1
              ? t("item.selectedOfCount", {
                  ordinal: options.selectionOrdinal,
                  count: options.selectedCount,
                })
              : t("item.selected"),
        };

  const status = workStatus(item.derivedWork, t);

  const relationshipText: string[] = [];
  const relationshipLabels: string[] = [];
  if (item.copyCount > 1) {
    relationshipText.push(t("grid.copyCount", { count: item.copyCount }));
    relationshipLabels.push(t("textPreview.exactCopies", { count: item.copyCount }));
  }
  if (options.similarCount > 1) {
    relationshipText.push(t("item.similarCount", { count: options.similarCount }));
    relationshipLabels.push(
      t("item.similarPhotosCount", { count: options.similarCount }),
    );
  }
  if (item.hasCompanions) {
    relationshipText.push(t("grid.companionBadge"));
    relationshipLabels.push(t("item.companionLabel"));
  }
  const relationships =
    relationshipText.length === 0
      ? null
      : {
          text: relationshipText.join(" · "),
          label: relationshipLabels.join("; "),
          tone: "muted" as const,
        };

  const stars = options.showFaceStars ? faceStarRating(item.faceScore) : 0;
  const transcript = item.derivedWork.transcripts;
  const faceRating =
    stars === 0
      ? null
      : {
          stars,
          label: t("face.starsAdvisory", { count: stars }),
          tone: "primary" as const,
        };
  const analysis =
    faceRating !== null
      ? faceRating
      : transcript?.state === "ready" && transcript.hasValue
        ? {
            text: t("item.transcriptBadge"),
            label: t("item.transcriptAvailable"),
            tone: "primary" as const,
          }
        : null;

  return { selection, status, relationships, analysis };
}
