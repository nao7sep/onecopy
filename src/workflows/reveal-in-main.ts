import { useItemsStore } from "../state/items-store";
import { useComparisonStore } from "../state/comparison-store";
import { useQuickViewStore } from "../state/quick-view-store";
import { reportActionFailure } from "../state/notifications-store";
import { closeComparison } from "./comparison";
import { closeViewer } from "./quick-view";

/** Shared diagnostic navigation; the requesting modal owns its inline result. */
export async function revealInMain(path: string, isCurrent: () => boolean, onRevealed: () => void) {
  const canLeave = () => !useComparisonStore.getState().busy &&
    useComparisonStore.getState().pendingAction === null &&
    useQuickViewStore.getState().pendingDelete === null;
  if (!canLeave()) return "blocked" as const;
  const result = await useItemsStore.getState().revealPath(path, () => isCurrent() && canLeave());
  if (result !== "revealed") return result;
  // Closing the requesting modal precedes native focus restoration. The
  // existing view owners dispose their readers/windows and restore Preview.
  onRevealed();
  try {
    if (useQuickViewStore.getState().session !== null) await closeViewer();
    if (useComparisonStore.getState().open) await closeComparison();
  } catch (error) {
    reportActionFailure("reveal-view-close-failed", "The file was selected, but its viewing window could not be closed. Close it to return to Main.", error);
  }
  return result;
}
