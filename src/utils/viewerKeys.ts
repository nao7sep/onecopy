import { isComposingEvent } from "../hooks/useComposing";
import { isAudioFile } from "../models/items";
import { isEditableTarget } from "./shortcuts";

/** Shared transient-viewer dispatch policy. Native controls keep their
 * ordinary activation/navigation; viewing transitions remain viewer-owned. */
export function viewerOwnsKey(event: KeyboardEvent, kind: string | null, fileName: string): boolean {
  if (event.defaultPrevented || isComposingEvent(event) || isEditableTarget(event.target)
    || event.metaKey || event.ctrlKey || event.altKey) return false;
  if ([" ", "f", "F", "Escape"].includes(event.key)) return !event.shiftKey;
  if (event.key === "Delete" || event.key === "Backspace") return true;
  if (event.target instanceof Element && event.target.closest(
    "button, a[href], input, select, textarea, audio[controls], video[controls], [role='menu'], [role='slider']",
  ) !== null) return false;
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") return true;
  const media = kind === "video" || isAudioFile(fileName);
  if (event.key === "Enter") return media;
  return (kind !== "other" || media) && ["PageUp", "PageDown", "Home", "End"].includes(event.key);
}
