// Quick View is a selection-scoped journey. Its store owns only whether the
// transient surface is open; eligibility belongs at the edge that can read
// the current item selection.

import { itemKey, useItemsStore } from "../state/items-store";
import { useQuickViewStore } from "../state/quick-view-store";

/** Main and grid key layers share this exact routing decision. */
export function handleSpaceQuickView(event: {
  preventDefault: () => void;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): boolean {
  if (event.metaKey || event.ctrlKey || event.altKey) return false;
  const opened = openQuickViewFromMain();
  event.preventDefault();
  return opened;
}

export function openQuickViewFromMain(): boolean {
  const { selectedItem, selectedKeys, items } = useItemsStore.getState();
  if (
    selectedItem === null ||
    selectedKeys.size === 0 ||
    !items.some((item) => itemKey(item) === selectedItem)
  ) {
    useItemsStore.setState({ message: "Select an item to open Quick View." });
    return false;
  }
  useQuickViewStore.getState().show();
  return true;
}
