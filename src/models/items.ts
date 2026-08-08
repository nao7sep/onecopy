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

/** Uppercased extension for the no-thumbnail placeholder tile. */
export function extLabel(fileName: string): string {
  const dot = fileName.lastIndexOf(".");
  if (dot <= 0 || dot === fileName.length - 1) return "FILE";
  return fileName.slice(dot + 1).toUpperCase();
}
