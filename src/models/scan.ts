// User-facing projection of the scanner's typed durable progress. The core
// owns phase/checkpoint facts; words and compact formatting belong here.

import type { MessageKey } from "../i18n/catalogues";
import type { Translator } from "../i18n/translate";

export interface ScanProgress {
  phase: string;
  done: number;
  total: number;
  currentPath: string | null;
  discovered: number | null;
  bytesDone: number | null;
  bytesTotal: number | null;
  failures: number;
  nextPhase: string | null;
}

const PHASE_LABELS: Record<string, MessageKey> = {
  walk: "scan.phaseWalk",
  hash: "scan.phaseHash",
  extract: "scan.phaseExtract",
  resolve: "scan.phaseResolve",
  pair: "scan.phasePair",
  indexed: "scan.phaseIndexed",
};

const PHASE_DESCRIPTIONS: Record<string, MessageKey> = {
  walk: "scan.descriptionWalk",
  hash: "scan.descriptionHash",
  extract: "scan.descriptionExtract",
  resolve: "scan.descriptionResolve",
  pair: "scan.descriptionPair",
  indexed: "scan.descriptionIndexed",
};

/** A phase's words. A phase the catalogue does not name falls back to its own
 * token, which is why this resolves text instead of returning a key. */
export function phaseLabel(phase: string, t: Translator["t"]): string {
  const key = PHASE_LABELS[phase];
  return key === undefined ? phase.charAt(0).toUpperCase() + phase.slice(1) : t(key);
}

function leaf(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Compact status-bar words. Every number comes directly from the backend
 * snapshot; phase-specific work is never inferred from a detail string.
 *
 * The facts are a variable join of independently translated parts, so this
 * composes them with the translator rather than returning one descriptor. */
export function progressLine(
  progress: ScanProgress,
  t: Translator["t"],
  percentOf: Translator["percent"],
): string {
  if (progress.phase === "indexed") {
    return t("work.noWork");
  }

  const parts: string[] = [];
  if (progress.phase === "walk") {
    const source = Math.min(progress.done + 1, progress.total);
    if (progress.total > 0) {
      parts.push(t("scan.sourceProgress", { done: source, total: progress.total }));
    }
    if (progress.discovered !== null) {
      parts.push(t("scan.filesFound", { count: progress.discovered }));
    }
    if (progress.currentPath !== null) parts.push(progress.currentPath);
  } else {
    parts.push(t("scan.doneOfTotal", { done: progress.done, total: progress.total }));
    if (progress.currentPath !== null && progress.done < progress.total) {
      parts.push(leaf(progress.currentPath));
    }
    if (
      progress.bytesDone !== null &&
      progress.bytesTotal !== null &&
      progress.bytesTotal > 0
    ) {
      const percent = Math.min(
        100,
        Math.floor((progress.bytesDone * 100) / progress.bytesTotal),
      );
      parts.push(percentOf(percent / 100));
    }
  }
  if (progress.nextPhase !== null && progress.done === progress.total) {
    parts.push(t("scan.nextPhase", { phase: phaseLabel(progress.nextPhase, t) }));
  }
  return t("scan.progress", {
    phase: phaseLabel(progress.phase, t),
    facts: parts.join(" · "),
  });
}

export function progressTitle(progress: ScanProgress, t: Translator["t"]): string {
  const key = PHASE_DESCRIPTIONS[progress.phase];
  const description = key === undefined ? phaseLabel(progress.phase, t) : t(key);
  return progress.nextPhase === null
    ? description
    : t("scan.titleWithNext", {
        description,
        phase: phaseLabel(progress.nextPhase, t),
      });
}
