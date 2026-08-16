// Mirrors queries::SectionItem on the Rust side.

import { convertFileSrc } from "@tauri-apps/api/core";

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
}

export type SortOrder = "time" | "name" | "size" | "resolution";

export function sortItems(items: SectionItem[], order: SortOrder): SectionItem[] {
  const sorted = [...items];
  switch (order) {
    case "time":
      sorted.sort(
        (a, b) =>
          (a.resolvedUtcMs ?? Number.MAX_SAFE_INTEGER) -
            (b.resolvedUtcMs ?? Number.MAX_SAFE_INTEGER) || a.pathId - b.pathId,
      );
      break;
    case "name":
      sorted.sort((a, b) =>
        a.fileName.toLowerCase().localeCompare(b.fileName.toLowerCase()),
      );
      break;
    case "size":
      sorted.sort((a, b) => (b.byteSize ?? -1) - (a.byteSize ?? -1));
      break;
    case "resolution":
      sorted.sort(
        (a, b) => (b.width ?? 0) * (b.height ?? 0) - (a.width ?? 0) * (a.height ?? 0),
      );
      break;
  }
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
