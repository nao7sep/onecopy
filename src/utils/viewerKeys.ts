import { isComposingEvent } from "../hooks/useComposing";
import { isAudioFile } from "../models/items";
import { isEditableTarget } from "./shortcuts";

/** Read-only transcript scrolling stays local; viewing and deletion do not. */
export function transcriptOwnsScrollKey(event: KeyboardEvent): boolean {
  return !event.defaultPrevented && !isComposingEvent(event) &&
    !event.metaKey && !event.ctrlKey && !event.altKey &&
    ["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End"].includes(event.key) &&
    event.target instanceof Element && event.target.closest("[data-transcript-scroll]") !== null;
}

/** Does a focused native or body-specific control already own this key?
 * Shared by the fullscreen view and the preview window, which both
 * forward navigation/activation keys to their command owner unless a real
 * control (a button, a native `<audio controls>`/`<video controls>` player, a
 * slider, a menu, or the transcript's own scroll region) already consumes
 * them. Delete/Backspace are NOT included here: the spec has only a genuinely
 * editable control consume deletion keys, which `isEditableTarget` already
 * decides on its own. */
export function controlOwnsForwardableKey(event: KeyboardEvent): boolean {
  if (transcriptOwnsScrollKey(event)) return true;
  return event.target instanceof Element && event.target.closest(
    "button, a[href], input, select, textarea, audio[controls], video[controls], [role='menu'], [role='slider']",
  ) !== null;
}

/** The fullscreen view's dispatch policy. Native controls keep their ordinary
 * activation/navigation; closing and deletion remain the view's. */
export function viewerOwnsKey(event: KeyboardEvent, kind: string | null, fileName: string): boolean {
  if (event.defaultPrevented || isComposingEvent(event) || isEditableTarget(event.target)
    || event.metaKey || event.ctrlKey || event.altKey) return false;
  if (event.key === " " || event.key === "Escape") return !event.shiftKey;
  if (event.key === "Delete" || event.key === "Backspace") return true;
  if (controlOwnsForwardableKey(event)) return false;
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") return true;
  const media = kind === "video" || isAudioFile(fileName);
  if (event.key === "Enter") return media;
  return (kind !== "other" || media) && ["PageUp", "PageDown", "Home", "End"].includes(event.key);
}
