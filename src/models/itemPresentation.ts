import { formatLocalMinute } from "../utils/displayTime";
import type { ItemDetail, ItemWorkState, ItemWorkStates, SectionItem } from "./items";

export type PresentationTone = "muted" | "primary" | "warning" | "danger";

export interface PresentationBadge {
  text: string;
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
  analysis: PresentationBadge | null;
}

const WORK_LABELS: Record<keyof ItemWorkStates, string> = {
  preview: "Preview",
  snapshots: "Video snapshots",
  similarity: "Similar photos",
  faces: "Face scoring",
  transcripts: "Transcription",
};

const WORK_SHORT_LABELS: Record<keyof ItemWorkStates, string> = {
  preview: "Preview",
  snapshots: "Snapshots",
  similarity: "Similarity",
  faces: "Faces",
  transcripts: "Transcript",
};

const WORK_ORDER = [
  "preview",
  "snapshots",
  "similarity",
  "faces",
  "transcripts",
] as const satisfies readonly (keyof ItemWorkStates)[];

export function takenPresentation(detail: ItemDetail): string {
  if (detail.dateState === "pending") return "Date pending";
  if (detail.resolvedUtcMs === null) return "Undated";
  const suffix = detail.dateOnly ? " (date only)" : "";
  const source = detail.resolvedSource ? ` · ${detail.resolvedSource}` : "";
  return `${formatLocalMinute(detail.resolvedUtcMs)}${suffix}${source}`;
}

function progressSuffix(state: ItemWorkState): string {
  if (state.done === null || state.total === null || state.total <= 0) return "";
  if (state.total === 100) return ` ${Math.min(100, Math.round(state.done))}%`;
  return ` ${state.done}/${state.total}`;
}

function readyWithoutValue(id: keyof ItemWorkStates): string {
  if (id === "faces") return "Checked — no face detected";
  if (id === "transcripts") return "Checked — no speech found";
  if (id === "similarity") return "Checked — no similar photos";
  if (id === "snapshots") return "Ready — no snapshots";
  return "Ready";
}

export function workPresentationRows(states: ItemWorkStates): WorkPresentationRow[] {
  return WORK_ORDER.flatMap((id) => {
    const state = states[id];
    if (state === null) return [];
    let value: string;
    let tone: PresentationTone = "muted";
    if (state.state === "ready") {
      value = state.hasValue ? "Ready" : readyWithoutValue(id);
    } else if (state.state === "running") {
      value = `${state.reason ?? "Running"}${progressSuffix(state)}`;
      tone = "primary";
    } else if (state.state === "failed") {
      value = state.reason ?? "Failed";
      tone = "danger";
    } else if (state.state === "pending") {
      value = "Queued";
    } else {
      value = state.reason ?? {
        disabled: "Off",
        unavailable: "Unavailable",
        blocked: "Blocked",
        waiting: "Waiting",
      }[state.state];
      tone = state.state === "disabled" ? "muted" : "warning";
    }
    return [{ id, label: WORK_LABELS[id], value, tone }];
  });
}

function workStatus(states: ItemWorkStates): PresentationBadge | null {
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
  const short = WORK_SHORT_LABELS[id];
  if (state.state === "failed") {
    return { text: `${short} failed`, label: `${WORK_LABELS[id]} failed`, tone: "danger" };
  }
  if (state.state === "running") {
    const suffix = progressSuffix(state);
    return {
      text: `${short}${suffix || "…"}`,
      label: `${WORK_LABELS[id]} running${suffix}`,
      tone: "primary",
    };
  }
  return {
    text: state.state === "pending" ? `${short} queued` : `${short} waiting`,
    label: `${WORK_LABELS[id]}: ${state.reason ?? (state.state === "pending" ? "Queued" : "Waiting")}`,
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

export function faceStarLabel(stars: number): string {
  return `${stars} face ${stars === 1 ? "star" : "stars"} — best-face confidence and smile hint`;
}

export function itemPresentation(
  item: SectionItem,
  options: {
    similarCount: number;
    selectionOrdinal: number | null;
    selectedCount: number;
    showFaceStars: boolean;
  },
): ItemPresentation {
  const selection =
    options.selectionOrdinal === null
      ? null
      : {
          ordinal: options.selectedCount > 1 ? options.selectionOrdinal : null,
          label:
            options.selectedCount > 1
              ? `Selected ${options.selectionOrdinal} of ${options.selectedCount}`
              : "Selected",
        };

  const status = workStatus(item.derivedWork);

  const relationshipText: string[] = [];
  const relationshipLabels: string[] = [];
  if (item.copyCount > 1) {
    relationshipText.push(`×${item.copyCount}`);
    relationshipLabels.push(`${item.copyCount} exact copies`);
  }
  if (options.similarCount > 1) {
    relationshipText.push(`≈${options.similarCount}`);
    relationshipLabels.push(`${options.similarCount} similar photos`);
  }
  if (item.hasCompanions) {
    relationshipText.push("pair");
    relationshipLabels.push("has paired companion files; every action includes them");
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
  const analysis =
    stars > 0
      ? { text: "★".repeat(stars), label: faceStarLabel(stars), tone: "primary" as const }
      : transcript?.state === "ready" && transcript.hasValue
        ? { text: "CC", label: "Transcript available", tone: "primary" as const }
        : null;

  return { selection, status, relationships, analysis };
}
