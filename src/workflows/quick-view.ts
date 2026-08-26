// Quick View is a selection-scoped journey. Its store owns only whether the
// transient surface is open; eligibility belongs at the edge that can read
// the current item selection.

import { itemKey, useItemsStore } from "../state/items-store";
import { useQuickViewStore } from "../state/quick-view-store";

function focusedVideoOwnsSpace(): boolean {
  if (typeof document === "undefined" || typeof Element === "undefined") return false;
  const active = document.activeElement;
  return active instanceof Element && active.closest("[data-video-surface]") !== null;
}

/** Main and grid key layers share this exact routing decision. */
export function handleSpaceQuickView(event: {
  preventDefault: () => void;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): boolean {
  if (event.metaKey || event.ctrlKey || event.altKey) return false;
  if (focusedVideoOwnsSpace()) return false;
  const { selected, selectedItem, items } = useItemsStore.getState();
  if (selected?.kind !== "image" && selected?.kind !== "video") return false;
  if (
    selectedItem === null ||
    !items.some((item) => itemKey(item) === selectedItem)
  ) {
    return false;
  }
  useQuickViewStore.getState().show();
  event.preventDefault();
  return true;
}
