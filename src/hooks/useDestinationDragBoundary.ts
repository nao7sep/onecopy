import { useEffect } from "react";
import { useDestinationsStore } from "../state/destinations-store";

const EDITABLE_SELECTOR =
  "textarea, [contenteditable='true'], input:not([type]), input[type='text'], input[type='search'], input[type='url'], input[type='email'], input[type='number'], input[type='password'], input[type='tel']";

export function keepsNativeTextDrop(event: DragEvent): boolean {
  // An app-owned pointer gesture never becomes editor text if a webview
  // happens to synthesize a native drag event alongside it.
  if (useDestinationsStore.getState().dragSelection !== null) return false;
  const target = event.target as Element | null;
  if (!target?.closest?.(EDITABLE_SELECTOR)) return false;
  const types = Array.from(event.dataTransfer?.types ?? []);
  if (types.includes("Files")) return false;
  return types.some(
    (type) =>
      type === "text/plain" || type === "text/uri-list" || type === "text/html",
  );
}

/**
 * App-wide native-drop safety.
 *
 * OneCopy has no native file or URL receiver: library sections are indexed
 * views and destination rows accept only app-owned indexed items. Native
 * offers are consumed invisibly so the webview cannot navigate, while
 * ordinary text/link editing remains browser-native in real editors.
 */
export function useDestinationDragBoundary(): void {
  useEffect(() => {
    const denyUnhandled = (event: DragEvent) => {
      if (keepsNativeTextDrop(event)) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "none";
    };
    const finishUnhandled = (event: DragEvent) => {
      if (!keepsNativeTextDrop(event)) event.preventDefault();
    };

    window.addEventListener("dragover", denyUnhandled);
    window.addEventListener("drop", finishUnhandled);
    return () => {
      window.removeEventListener("dragover", denyUnhandled);
      window.removeEventListener("drop", finishUnhandled);
    };
  }, []);
}
