// Mirrors queries::SectionItem on the Rust side.

import { convertFileSrc } from "@tauri-apps/api/core";

export type ItemWorkStatus =
  | "disabled"
  | "unavailable"
  | "blocked"
  | "waiting"
  | "pending"
  | "running"
  | "ready"
  | "failed";

export interface ItemWorkState {
  state: ItemWorkStatus;
  hasValue: boolean;
  reason: string | null;
  done: number | null;
  total: number | null;
}

export interface ItemWorkStates {
  preview: ItemWorkState | null;
  snapshots: ItemWorkState | null;
  similarity: ItemWorkState | null;
  faces: ItemWorkState | null;
  transcripts: ItemWorkState | null;
}

export const EMPTY_ITEM_WORK: ItemWorkStates = {
  preview: null,
  snapshots: null,
  similarity: null,
  faces: null,
  transcripts: null,
};

export interface SectionItem {
  hash: string | null;
  pathId: number;
  fileName: string;
  resolvedUtcMs: number | null;
  copyCount: number;
  width: number | null;
  height: number | null;
  hasThumb: boolean;
  similarGroupId: number | null;
  sharpness: number | null;
  byteSize: number | null;
  hasCompanions: boolean;
  durationMs: number | null;
  /** This binary exists under more than one file name across its copies —
   * move/copy are blocked (which name lands cannot be a surprise). */
  namesDiffer: boolean;
  /** Every live copy's directory, deduped, sorted (display form). */
  dirPaths: string[];
  derivedWork: ItemWorkStates;
}

/** Mirrors queries::ItemDetail on the Rust side. */
export interface ItemDetail {
  fileName: string;
  kind: string;
  byteSize: number | null;
  width: number | null;
  height: number | null;
  durationMs: number | null;
  resolvedUtcMs: number | null;
  resolvedSource: string | null;
  dateOnly: boolean;
  copyPaths: string[];
  companionPaths: string[];
  stripFrames: number | null;
}

/** Matches the backend's evenly spaced interior scene timestamps exactly. */
export function stripTimestampMs(durationMs: number, count: number, index: number): number {
  if (count <= 0) return 0;
  return Math.floor((Math.max(0, durationMs) * (index + 1)) / (count + 1));
}

export function timestampLabel(milliseconds: number): string {
  const seconds = Math.floor(Math.max(0, milliseconds) / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** Replaces one derived logical row without re-reading its whole section.
 * Identity promotion may collapse into a canonical row already present, so
 * both keys are removed before the one current row is inserted. */
export function replaceDerivedItem(
  items: SectionItem[],
  previousHash: string,
  item: SectionItem,
): SectionItem[] {
  if (item.hash === null) return items;
  const affected = items
    .map((candidate, index) =>
      candidate.hash === previousHash || candidate.hash === item.hash ? index : -1,
    )
    .filter((index) => index >= 0);
  if (affected.length === 0) return items;
  const insertion = Math.min(...affected);
  const next = items.filter(
    (candidate) => candidate.hash !== previousHash && candidate.hash !== item.hash,
  );
  next.splice(Math.min(insertion, next.length), 0, item);
  return next;
}

export type SortOrder = "time" | "name" | "size" | "resolution" | "ext";

export interface SortChoice {
  order: SortOrder;
  desc: boolean;
}

/** The extension, lowercased, for sorting — "" when the name has none. */
export function extOf(fileName: string): string {
  const dot = fileName.lastIndexOf(".");
  return dot > 0 ? fileName.slice(dot + 1).toLowerCase() : "";
}

/** Sort is CONTEXT-AWARE (the developer's Finder/Explorer expectation):
 * "time" means the capture time for photos and videos but is a nonsense
 * label for other-files, whose date is filesystem-derived; "resolution" only
 * exists for media at all. Each kind offers its own orders and default —
 * the labels live beside them so a menu cannot offer an order the kind
 * cannot honour. Folder sort is deliberately ABSENT: copies merge into one
 * row with many folders, so any single sort key was an arbitrary pick. */
export const SORT_ORDERS: Record<
  "media" | "other",
  { orders: Partial<Record<SortOrder, string>>; defaultChoice: SortChoice }
> = {
  media: {
    orders: { time: "Time taken", name: "Name", size: "Size", resolution: "Resolution" },
    defaultChoice: { order: "time", desc: false },
  },
  other: {
    orders: { name: "Name", ext: "Kind", size: "Size", time: "Date" },
    defaultChoice: { order: "name", desc: false },
  },
};

/** The direction a FRESH pick of each order starts with — what a person
 * means by the bare words: "sort by time" walks history forward, but "sort
 * by size" means biggest first. */
export const DEFAULT_DESC: Record<SortOrder, boolean> = {
  time: false,
  name: false,
  size: true,
  resolution: true,
  ext: false,
};

/** One primary comparator per order, ASCENDING; direction is applied to the
 * primary alone. */
const COMPARE: Record<SortOrder, (a: SectionItem, b: SectionItem) => number> = {
  time: (a, b) =>
    (a.resolvedUtcMs ?? Number.MAX_SAFE_INTEGER) - (b.resolvedUtcMs ?? Number.MAX_SAFE_INTEGER),
  name: (a, b) => a.fileName.toLowerCase().localeCompare(b.fileName.toLowerCase()),
  size: (a, b) => (a.byteSize ?? -1) - (b.byteSize ?? -1),
  resolution: (a, b) => (a.width ?? 0) * (a.height ?? 0) - (b.width ?? 0) * (b.height ?? 0),
  ext: (a, b) => extOf(a.fileName).localeCompare(extOf(b.fileName)),
};

/** Tie-break chains (Phase 33): every order resolves its ties through a
 * defined sequence — same-resolution photos from one phone fall back to
 * shooting order, then name — with pathId as the immutable final key, so
 * every sort is total and stable. Chained keys stay ASCENDING even under a
 * descending primary, the way Finder reads: resolution-descending still
 * shows each resolution group in shooting order. */
const CHAINS: Record<SortOrder, SortOrder[]> = {
  time: ["name"],
  name: ["time"],
  size: ["time", "name"],
  resolution: ["time", "name"],
  ext: ["name"],
};

export function sortItems(items: SectionItem[], choice: SortChoice): SectionItem[] {
  const sorted = [...items];
  const primary = COMPARE[choice.order];
  const chain = CHAINS[choice.order].map((order) => COMPARE[order]);
  sorted.sort((a, b) => {
    const head = primary(a, b);
    if (head !== 0) return choice.desc ? -head : head;
    for (const compare of chain) {
      const next = compare(a, b);
      if (next !== 0) return next;
    }
    return a.pathId - b.pathId;
  });
  return sorted;
}

// The mediacache protocol serves the hash-keyed cache; convertFileSrc builds
// the platform-correct URL (mediacache://localhost/… on macOS,
// http://mediacache.localhost/… on Windows).
export function thumbUrl(hash: string): string {
  return convertFileSrc(`thumb-${hash}`, "mediacache");
}

export function previewUrl(hash: string): string {
  return convertFileSrc(`preview-${hash}`, "mediacache");
}

export function stripUrl(hash: string, index: number): string {
  return convertFileSrc(`strip-${hash}-${index}`, "mediacache");
}

// The range-capable original-file protocol: video playback, audio playback,
// and the 100% view. Hash-keyed when a hash exists; `path-<id>` otherwise —
// an audio memo with a unique size is never content-read, so it may live its
// whole life unhashed.
export function originalUrl(hash: string): string {
  return convertFileSrc(hash, "mediafile");
}

export function originalUrlByPath(pathId: number): string {
  return convertFileSrc(`path-${pathId}`, "mediafile");
}

/** Formats the webview cannot paint from original bytes (WebView2 paints
 * neither; every platform routes the 100% view through the converted cache
 * entry so behaviour never differs by OS). */
const CONVERTED_FULLRES_EXTENSIONS = new Set(["heic", "heif", "hif", "avif"]);

export function needsConvertedFullres(fileName: string): boolean {
  const dot = fileName.lastIndexOf(".");
  if (dot <= 0 || dot === fileName.length - 1) return false;
  return CONVERTED_FULLRES_EXTENSIONS.has(fileName.slice(dot + 1).toLowerCase());
}

export function fullresUrl(hash: string): string {
  return convertFileSrc(`fullres-${hash}`, "mediacache");
}

/** Audio detection by extension — the file stays an OTHER-file everywhere
 * (list view, sections); only the preview treats it specially, by playing
 * it instead of showing a blank surface. */
const AUDIO_EXTENSIONS = new Set([
  "mp3", "m4a", "aac", "wav", "aiff", "aif", "flac", "ogg", "oga", "opus", "amr",
]);

export function isAudioFile(fileName: string): boolean {
  const dot = fileName.lastIndexOf(".");
  if (dot <= 0 || dot === fileName.length - 1) return false;
  return AUDIO_EXTENSIONS.has(fileName.slice(dot + 1).toLowerCase());
}

/** `m:ss` / `h:mm:ss` for duration badges. */
export function formatDuration(durationMs: number): string {
  const totalSeconds = Math.round(durationMs / 1000);
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  const two = (n: number) => n.toString().padStart(2, "0");
  return hours > 0
    ? `${hours}:${two(minutes)}:${two(seconds)}`
    : `${minutes}:${two(seconds)}`;
}

/** Compact byte size for tiles and list rows — one decimal below 10 units so
 * "1.4 MB" and "940 KB" both stay short. Binary units, as every file manager
 * on both platforms reports them. */
export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = unit === 0 || value >= 10 ? Math.round(value) : Math.round(value * 10) / 10;
  return `${rounded} ${units[unit]}`;
}

/** `4032×3024`, or null when the item carries no dimensions (an other-file, or
 * a still whose decode is still blocked on ffmpeg). */
export function formatDimensions(
  width: number | null,
  height: number | null,
): string | null {
  if (width === null || height === null) return null;
  return `${width}×${height}`;
}

/** The one line of hard facts a tile or row shows beneath the name: pixels
 * then bytes, whichever of the two is known. */
export function factsLine(item: {
  width: number | null;
  height: number | null;
  byteSize: number | null;
}): string {
  return [formatDimensions(item.width, item.height), item.byteSize !== null ? formatBytes(item.byteSize) : null]
    .filter((part): part is string => part !== null)
    .join(" · ");
}

/** Uppercased extension for the no-thumbnail placeholder tile. */
export function extLabel(fileName: string): string {
  const dot = fileName.lastIndexOf(".");
  if (dot <= 0 || dot === fileName.length - 1) return "FILE";
  return fileName.slice(dot + 1).toUpperCase();
}
