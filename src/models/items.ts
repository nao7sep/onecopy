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

/** Uppercased extension for the no-thumbnail placeholder tile. */
export function extLabel(fileName: string): string {
  const dot = fileName.lastIndexOf(".");
  if (dot <= 0 || dot === fileName.length - 1) return "FILE";
  return fileName.slice(dot + 1).toUpperCase();
}
