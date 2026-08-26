// Main-shell keyboard policy and the confirmation state it creates. Rendered
// dialogs stay in App, while their command semantics have one owner here.

import { useCallback, useEffect, useState } from "react";
import { comparisonHashForEnter } from "../models/interactions";
import { useAppStore } from "../state/app-store";
import { itemKey, useItemsStore } from "../state/items-store";
import { useComparisonStore } from "../state/comparison-store";
import { useSettingsStore } from "../state/settings-store";
import { hasOpenModal } from "../utils/modalStack";
import {
  isEditableTarget,
  isHelpShortcut,
  isSettingsShortcut,
  shadowsMacTextEditing,
} from "../utils/shortcuts";
import { openComparison } from "../workflows/comparison";
import { deleteSelectedItems } from "../workflows/items";
import { handleSpaceQuickView } from "../workflows/quick-view";

export function useGlobalCommands() {
  const [helpOpen, setHelpOpen] = useState(false);
  const [confirmPermanent, setConfirmPermanent] = useState<number | null>(null);
  const [confirmTrash, setConfirmTrash] = useState<number | null>(null);

  const openSettings = useCallback(() => {
    useSettingsStore.getState().openWith(useAppStore.getState().appData?.config ?? null);
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const editable = isEditableTarget(event.target);
      if (editable && (event.key === "?" || shadowsMacTextEditing(event))) return;
      if (isHelpShortcut(event)) {
        event.preventDefault();
        setHelpOpen((open) => (open ? false : hasOpenModal() ? open : true));
      } else if (isSettingsShortcut(event)) {
        event.preventDefault();
        if (!hasOpenModal()) openSettings();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [openSettings]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (hasOpenModal() || useComparisonStore.getState().open) return;
      if (event.defaultPrevented || isEditableTarget(event.target)) return;
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        const { selectedKeys, selectedItem } = useItemsStore.getState();
        const count = selectedKeys.size > 0 ? selectedKeys.size : selectedItem !== null ? 1 : 0;
        if (count === 0) return;
        if (event.shiftKey) {
          setConfirmPermanent(count);
        } else if (useAppStore.getState().appData?.config?.confirmTrashDelete === true) {
          setConfirmTrash(count);
        } else {
          void deleteSelectedItems(false);
        }
      } else if (
        event.key === " " &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey
      ) {
        handleSpaceQuickView(event);
      } else if (event.key === "Enter") {
        const { items, selectedItem } = useItemsStore.getState();
        const item = items.find((candidate) => itemKey(candidate) === selectedItem);
        if (item === undefined) return;
        event.preventDefault();
        const comparisonHash = comparisonHashForEnter(item);
        if (comparisonHash === null) return;
        void openComparison(comparisonHash).then((opened) => {
          if (opened) return;
          useItemsStore.setState({ message: "No similar photos left in this group" });
        });
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return {
    helpOpen,
    openHelp: () => setHelpOpen(true),
    closeHelp: () => setHelpOpen(false),
    openSettings,
    confirmPermanent,
    confirmTrash,
    cancelPermanentDelete: () => setConfirmPermanent(null),
    cancelTrashDelete: () => setConfirmTrash(null),
    confirmPermanentDelete: () => {
      setConfirmPermanent(null);
      void deleteSelectedItems(true);
    },
    confirmTrashDelete: () => {
      setConfirmTrash(null);
      void deleteSelectedItems(false);
    },
  };
}
