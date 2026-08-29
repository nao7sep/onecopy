// User-facing projection of the scanner's typed durable progress. The core
// owns phase/checkpoint facts; words and compact formatting belong here.

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

const PHASE_LABELS: Record<string, string> = {
  walk: "Checking source folders",
  hash: "Reading files",
  extract: "Reading metadata",
  resolve: "Working out dates",
  pair: "Pairing companions",
  indexed: "Indexed",
};

const PHASE_DESCRIPTIONS: Record<string, string> = {
  walk: "Checks configured folders and records the files currently present.",
  hash:
    "Reads file bytes only when identity cannot be decided from existing facts. Cloud placeholders may download while read.",
  extract:
    "Reads EXIF and media-container structures without decoding image pixels or video frames. Cloud providers or containers may still hydrate or seek the file.",
  resolve: "Chooses each file's display date from saved metadata, filename, and filesystem facts.",
  pair: "Connects RAW sidecars and Live Photo companions to their primary media.",
  indexed:
    "The durable index is up to date. Previews, snapshots, similarity, faces, and transcripts continue separately in Background work.",
};

export function phaseLabel(phase: string): string {
  return PHASE_LABELS[phase] ?? phase.charAt(0).toUpperCase() + phase.slice(1);
}

function count(value: number): string {
  return value.toLocaleString();
}

function leaf(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Compact status-bar words. Every number comes directly from the backend
 * snapshot; phase-specific work is never inferred from a detail string. */
export function progressLine(progress: ScanProgress): string {
  if (progress.phase === "indexed") {
    return progress.failures > 0
      ? `Indexed — ${count(progress.failures)} failed · open Issues`
      : "Up to date";
  }

  const parts: string[] = [];
  if (progress.phase === "walk") {
    const source = Math.min(progress.done + 1, progress.total);
    if (progress.total > 0) {
      parts.push(`source ${count(source)}/${count(progress.total)}`);
    }
    if (progress.discovered !== null) {
      parts.push(
        `${count(progress.discovered)} file${progress.discovered === 1 ? "" : "s"} found`,
      );
    }
    if (progress.currentPath !== null) parts.push(progress.currentPath);
  } else {
    parts.push(`${count(progress.done)}/${count(progress.total)}`);
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
      parts.push(`${percent}%`);
    }
  }
  if (progress.failures > 0) parts.push(`${count(progress.failures)} failed`);
  if (progress.nextPhase !== null && progress.done === progress.total) {
    parts.push(`Next: ${phaseLabel(progress.nextPhase)}`);
  }
  return `${phaseLabel(progress.phase)} \u2014 ${parts.join(" · ")}`;
}

export function progressTitle(progress: ScanProgress): string {
  const description =
    progress.phase === "indexed" && progress.failures > 0
      ? `Indexing finished with ${count(progress.failures)} failed file${progress.failures === 1 ? "" : "s"}; open Issues to inspect them. Previews, snapshots, similarity, faces, and transcripts continue separately in Background work.`
      : (PHASE_DESCRIPTIONS[progress.phase] ?? phaseLabel(progress.phase));
  return progress.nextPhase === null
    ? description
    : `${description} Next: ${phaseLabel(progress.nextPhase)}.`;
}
