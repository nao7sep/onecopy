// Main-shell keyboard policy and the confirmation state it creates. Rendered
// dialogs stay in App, while their command semantics have one owner here.

import { useCallback, useEffect, useState } from "react";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { useComparisonStore } from "../state/comparison-store";
import { useSettingsStore } from "../state/settings-store";
import { useSectionsStore } from "../state/sections-store";
import { hasOpenModal } from "../utils/modalStack";
import {
  isEditableTarget,
  isHelpShortcut,
  isSectionRecheckShortcut,
  isSettingsShortcut,
  shadowsMacTextEditing,
} from "../utils/shortcuts";
import { requestComparisonFromMain } from "../workflows/comparison";
import { deleteSelectedItems, rescanCurrentSection } from "../workflows/items";
import { handleFViewer, handleSpaceQuickView } from "../workflows/quick-view";
import { isAudioFile, sectionProjection } from "../models/items";
import { toggleMainPlayback } from "../workflows/playback";

export function useGlobalCommands() {
  const [helpOpen, setHelpOpen] = useState(false);
  const [confirmPermanent, setConfirmPermanent] = useState<number | null>(null);
  const [confirmTrash, setConfirmTrash] = useState<number | null>(null);

  const openSettings = useCallback(() => {
    const appData = useAppStore.getState().appData;
    useSettingsStore.getState().openWith(
      appData?.config ?? null,
      appData?.state ?? null,
    );
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const editable = isEditableTarget(event.target);
      if (editable && (event.key === "?" || shadowsMacTextEditing(event)))
        return;
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
      if (isSectionRecheckShortcut(event)) {
        event.preventDefault();
        if (!useSectionsStore.getState().sourceCheck.running) {
          void rescanCurrentSection();
        }
      } else if (event.key === "Delete" || event.key === "Backspace") {
        if (
          !(event.target instanceof Element) ||
          event.target.closest("#main-item-area") === null
        ) {
          return;
        }
        event.preventDefault();
        const { selectedKeys, selectedItem } = useItemsStore.getState();
        const count =
          selectedKeys.size > 0
            ? selectedKeys.size
            : selectedItem !== null
              ? 1
              : 0;
        if (count === 0) return;
        if (event.shiftKey) {
          setConfirmPermanent(count);
        } else if (
          useAppStore.getState().appData?.config?.confirmTrashDelete === true
        ) {
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
        if (
          !(event.target instanceof Element) ||
          event.target.closest("#main-item-area") === null
        ) {
          return;
        }
        handleSpaceQuickView(event);
      } else if (
        event.key.toLowerCase() === "f" &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey
      ) {
        if (
          !(event.target instanceof Element) ||
          event.target.closest("#main-item-area") === null
        ) {
          return;
        }
        handleFViewer(event);
      } else if (event.key === "Enter") {
        if (
          !(event.target instanceof Element) ||
          event.target.closest("#main-item-area") === null
        ) {
          return;
        }
        const items = useItemsStore.getState();
        if (items.selected?.kind === "image") {
          event.preventDefault();
          void requestComparisonFromMain();
          return;
        }
        const anchor =
          items.selectedItem === null
            ? undefined
            : sectionProjection(items.items, items.currentSort()).itemByKey.get(
                items.selectedItem,
              );
        if (
          items.selected?.kind === "video" ||
          (anchor !== undefined && isAudioFile(anchor.fileName))
        ) {
          event.preventDefault();
          if (
            items.selectedItem !== null &&
            !toggleMainPlayback(items.selectedItem)
          ) {
            useItemsStore.setState({
              message: "This item is not playable in OneCopy right now.",
            });
          }
        }
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
