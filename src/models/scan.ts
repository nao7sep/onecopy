// User-facing labels for the scan pipeline's phase tokens (the core emits
// stable internal tokens; words belong to the UI). The fallback capitalizes
// an unknown token so a new phase degrades to readable rather than raw.

const PHASE_LABELS: Record<string, string> = {
  walk: "Scanning",
  hash: "Reading files",
  extract: "Reading file info",
  resolve: "Working out dates",
  pair: "Pairing companions",
  derive: "Making previews",
  video: "Making video previews",
  embed: "Comparing photos",
  faces: "Scoring faces",
  group: "Grouping similar shots",
};

export function phaseLabel(phase: string): string {
  return PHASE_LABELS[phase] ?? phase.charAt(0).toUpperCase() + phase.slice(1);
}

/** One status-bar line for a progress event: friendly label, then the
 * detail, dash-joined so a detail carrying its own colon still reads. */
export function progressLine(phase: string, detail: string): string {
  return `${phaseLabel(phase)} \u2014 ${detail}`;
}
