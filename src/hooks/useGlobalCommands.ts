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
  isSettingsShortcut,
  shadowsMacTextEditing,
} from "../utils/shortcuts";
import { requestComparisonFromMain } from "../workflows/comparison";
import {
  captureDeleteSelection,
  deleteItems,
  rescanCurrentSection,
} from "../workflows/items";
import { handleFViewer, handleSpaceQuickView } from "../workflows/quick-view";
import { isAudioFile, itemKey } from "../models/items";
import { toggleMainPlayback } from "../workflows/playback";
import { isComposingEvent } from "./useComposing";
import { useQuickViewStore } from "../state/quick-view-store";
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
      if (hasOpenModal() || useComparisonStore.getState().open || useQuickViewStore.getState().session !== null || isComposingEvent(event)) return;
      if (event.defaultPrevented || isEditableTarget(event.target)) return;
      if (isSectionRecheckShortcut(event)) {
        event.preventDefault();
        if (!useSectionsStore.getState().sourceCheck.running) {
          void rescanCurrentSection();
        }
      } else if (event.key === "Delete" || event.key === "Backspace") {
        // The pane placement of persistent Preview is not a navigation
        // context, but it IS still "persistent Preview" for deletion scope
        // (viewing-sessions.md: pane and separate window are one follower
        // with two placements) — its read-only bodies forward Delete to
        // Main's complete-selection Trash exactly as the separate window's
        // own key forwarding does (D10).
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
        // Unlike Delete/Enter/Space, F is not exclusive to the main item
        // area: the in-pane Preview also owns an F path to true fullscreen
        // (viewing-sessions.md). Only the sidebar tree opts out, matching
        // "item commands do not act while the sidebar owns focus."
        if (event.target instanceof Element && event.target.closest('[role="tree"]') !== null) {
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
