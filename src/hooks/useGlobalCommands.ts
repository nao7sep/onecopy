// Main-shell keyboard policy and the confirmation state it creates. Rendered
// dialogs stay in App, while their command semantics have one owner here.

import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { useComparisonStore } from "../state/comparison-store";
import { useSettingsStore } from "../state/settings-store";
import { useAppShellStore } from "../state/app-shell-store";
import { useSectionsStore } from "../state/sections-store";
import { hasOpenModal } from "../utils/modalStack";
import {
  isEditableTarget,
  isHelpShortcut,
  isSectionRecheckShortcut,
  isSelectAllShortcut,
  isSettingsShortcut,
  shadowsMacTextEditing,
} from "../utils/shortcuts";
import { requestComparisonFromMain } from "../workflows/comparison";
import {
  captureDeleteSelection,
  deleteItems,
  rescanCurrentSection,
} from "../workflows/items";
import { handleSpaceFullscreenView } from "../workflows/fullscreen-view";
import { isAudioFile, itemKey } from "../models/items";
import { toggleMainPlayback } from "../workflows/playback";
import { isComposingEvent } from "./useComposing";
import { useFullscreenViewStore } from "../state/fullscreen-view-store";
import { confirmsTrashDelete } from "../models/config";

/** The exact ordered logical items a Main deletion review shows. */
interface DeleteReview {
  keys: readonly string[];
  permanent: boolean;
}

export function useGlobalCommands() {
  const [deleteReview, setDeleteReview] = useState<DeleteReview | null>(null);

  const openSettings = useCallback(() => {
    const appData = useAppStore.getState().appData;
    useSettingsStore.getState().beginEditing(
      appData?.config ?? null,
      appData?.aiAccelerationCapabilities ?? [],
      appData?.configDefaults ?? null,
    );
    useAppShellStore.getState().openUtility("settings");
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || isComposingEvent(event) || hasOpenModal()) return;
      const editable = isEditableTarget(event.target);
      if (editable && (event.key === "?" || shadowsMacTextEditing(event)))
        return;
      if (isHelpShortcut(event)) {
        event.preventDefault();
        if (!hasOpenModal()) useAppShellStore.getState().openUtility("shortcuts");
      } else if (isSettingsShortcut(event)) {
        event.preventDefault();
        if (!hasOpenModal()) openSettings();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [openSettings]);

  // The macOS native menu's Settings… item (R8-04) cannot call into the
  // webview directly; the core emits this event, and Main is the one owner of
  // the Settings surface, same as the webview's own Cmd+, shortcut above.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen("menu://open-settings", () => {
      if (!hasOpenModal()) openSettings();
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [openSettings]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // Comparison and the fullscreen view cover Main's list while they are
      // open, so a key reaching Main then must not delete, open or compare
      // items the user cannot see.
      if (hasOpenModal() || useComparisonStore.getState().open || useFullscreenViewStore.getState().session !== null || isComposingEvent(event)) return;
      if (event.defaultPrevented || isEditableTarget(event.target)) return;
      if (isSectionRecheckShortcut(event)) {
        event.preventDefault();
        if (!useSectionsStore.getState().sourceCheck.running) {
          void rescanCurrentSection();
        }
      } else if (isSelectAllShortcut(event)) {
        // The preview pane's read-only text keeps the chord for selecting its
        // own words, as the preview window's does, and an open menu owns
        // input; everywhere else in Main it selects the whole section, loaded
        // or not.
        if (
          event.target instanceof Element &&
          (event.target.closest("[data-preview-pane]") !== null ||
            event.target.closest("[role='menu']") !== null)
        ) {
          return;
        }
        event.preventDefault();
        if (event.repeat) return;
        void useItemsStore.getState().selectAll();
      } else if (event.key === "Delete" || event.key === "Backspace") {
        // The preview pane is not a navigation context, but it is the same
        // preview as the preview window, shown in another place: its
        // read-only bodies forward Delete to Main's complete-selection Trash
        // exactly as the window's own key forwarding does.
        if (
          !(event.target instanceof Element) ||
          (event.target.closest("#main-item-area") === null &&
            event.target.closest("[data-preview-pane]") === null)
        ) {
          return;
        }
        event.preventDefault();
        if (event.repeat) return;
        const keys = captureDeleteSelection();
        if (keys.length === 0) return;
        if (event.shiftKey) {
          setDeleteReview({ keys, permanent: true });
        } else if (
          keys.length > 1 ||
          confirmsTrashDelete(useAppStore.getState().appData?.config)
        ) {
          setDeleteReview({ keys, permanent: false });
        } else {
          void deleteItems(keys, false);
        }
      } else if (
        event.key === " " &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey
      ) {
        // Space, like Enter and Delete, acts only from Main's items: the
        // sidebar and other panes keep their own meaning for it.
        if (
          !(event.target instanceof Element) ||
          event.target.closest("#main-item-area") === null
        ) {
          return;
        }
        handleSpaceFullscreenView(event);
      } else if (event.key === "Enter") {
        if (
          !(event.target instanceof Element) ||
          event.target.closest("#main-item-area") === null
        ) {
          return;
        }
        event.preventDefault();
        if (event.repeat) return;
        const items = useItemsStore.getState();
        if (items.selected?.kind === "image") {
          void requestComparisonFromMain();
          return;
        }
        const anchor =
          items.selectedItem === null
            ? undefined
            : items.items.find((item) => itemKey(item) === items.selectedItem);
        if (
          items.selected?.kind === "video" ||
          (anchor !== undefined && isAudioFile(anchor.fileName))
        ) {
          // Enter does nothing when no player is visible for the anchor
          // (Preview closed, showing a different item, or not yet ready);
          // there is no failure to report in that case.
          if (items.selectedItem !== null) toggleMainPlayback(items.selectedItem);
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return {
    openHelp: () => useAppShellStore.getState().openUtility("shortcuts"),
    openSettings,
    confirmPermanent:
      deleteReview?.permanent === true ? deleteReview.keys.length : null,
    confirmTrash:
      deleteReview?.permanent === false ? deleteReview.keys.length : null,
    cancelPermanentDelete: () => setDeleteReview(null),
    cancelTrashDelete: () => setDeleteReview(null),
    confirmPermanentDelete: () => {
      if (deleteReview === null) return;
      setDeleteReview(null);
      void deleteItems(deleteReview.keys, true);
    },
    confirmTrashDelete: () => {
      if (deleteReview === null) return;
      setDeleteReview(null);
      void deleteItems(deleteReview.keys, false);
    },
  };
}
