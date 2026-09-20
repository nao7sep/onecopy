import type { MessageKey } from "../i18n/catalogues";
import { primaryModWord } from "../utils/shortcuts";

// Keystrokes stay in the keyboard's own English (keyboard-shortcut
// conventions); every word describing what a shortcut DOES is a catalogue key.
export interface ShortcutRow { chord: string; action: MessageKey }
export interface ShortcutGroup { title: MessageKey; context: MessageKey; rows: ShortcutRow[] }

/** Semantic reading columns: Main navigation, viewing, then decisions/app.
 * Narrow layouts retain that DOM order without splitting a group. */
export function shortcutColumns(): ShortcutGroup[][] {
  const mod = primaryModWord();
  const group = (
    title: MessageKey,
    context: MessageKey,
    rows: [string, MessageKey][],
  ): ShortcutGroup =>
    ({ title, context, rows: rows.map(([chord, action]) => ({ chord, action })) });
  return [
    [
      group("shortcuts.groupMainItems", "shortcuts.contextMainItems", [
        ["Arrows", "shortcuts.itemsMoveSelection"],
        ["Home/End", "shortcuts.itemsFirstLast"],
        ["PageUp/PageDown", "shortcuts.itemsByScreenful"],
        ["Shift+Arrows/Home/End/PageUp/PageDown", "shortcuts.extendSelectionRange"],
        ["Click", "shortcuts.itemsSelectOnly"],
        [mod + "+Click", "shortcuts.itemsToggleSelection"],
        ["Shift+Click", "shortcuts.extendSelectionRange"],
        ["Space", "shortcuts.itemsOpenQuickView"],
        ["Double-click", "shortcuts.itemsSelectAndQuickView"],
        ["Enter", "shortcuts.itemsCompareOrPlay"],
        ["F", "shortcuts.itemsOpenFullscreen"],
        ["Delete/Backspace", "shortcuts.itemsTrashSelection"],
        ["Shift+Delete/Backspace", "shortcuts.itemsReviewPermanentSelection"],
      ]),
      group("sidebar.sections", "shortcuts.contextSections", [
        ["Up/Down", "shortcuts.sectionsPreviousNextRow"],
        ["PageUp/PageDown", "shortcuts.sectionsTenRows"],
        ["Home/End", "shortcuts.sectionsFirstLastRow"],
        ["Left/Right", "shortcuts.treeCollapseExpand"],
        ["Enter/Space", "shortcuts.sectionsToggleBranch"],
        ["Right/Tab", "shortcuts.sectionsFocusMainItems"],
      ]),
      group("destinations.title", "shortcuts.contextDestinations", [
        ["Up/Down", "shortcuts.destinationsPreviousNext"],
        ["PageUp/PageDown", "shortcuts.destinationsTenFolders"],
        ["Home/End", "shortcuts.destinationsFirstLast"],
        ["Left/Right", "shortcuts.treeCollapseExpand"],
        ["Enter", "shortcuts.destinationsExpandOnly"],
      ]),
    ],
    [
      group("shortcuts.groupQuickView", "shortcuts.contextQuickView", [
        ["Space", "shortcuts.quickViewSpace"],
        ["F", "shortcuts.quickViewF"],
        ["Escape", "shortcuts.returnToMain"],
        ["Left/Right", "shortcuts.quickViewPreviousNextFile"],
        ["PageUp/PageDown", "shortcuts.quickViewPreviousNextMedia"],
        ["Home/End", "shortcuts.quickViewFirstLastMedia"],
        ["Delete/Backspace", "shortcuts.quickViewTrashDisplayed"],
        ["Shift+Delete/Backspace", "shortcuts.quickViewReviewPermanentDisplayed"],
      ]),
      group("shortcuts.groupPreviewWindow", "shortcuts.contextPreviewWindow", [
        ["F", "shortcuts.previewToggleFullscreen"],
        ["Escape", "shortcuts.previewLeaveOrClose"],
        ["Arrows/Home/End/PageUp/PageDown", "shortcuts.previewNavigateMain"],
        ["Delete/Backspace", "shortcuts.previewTrashMainSelection"],
        ["Shift+Delete/Backspace", "shortcuts.previewReviewPermanentMainSelection"],
      ]),
      group("shortcuts.groupMediaAndText", "shortcuts.contextMediaAndText", [
        ["Enter", "shortcuts.mediaPlayPause"],
        ["Up/Down/PageUp/PageDown/Home/End", "shortcuts.mediaScrollText"],
        [mod + "+A/C", "shortcuts.mediaSelectAllCopy"],
        ["Press and hold", "shortcuts.mediaInspectOriginal"],
      ]),
      group("shortcuts.groupConfirmations", "shortcuts.contextConfirmations", [
        ["Left/Right", "shortcuts.confirmAdjacentAction"],
        ["Tab", "shortcuts.confirmCancelToDelete"],
        ["Enter", "shortcuts.confirmActivate"],
        ["Escape", "common.cancel"],
      ]),
    ],
    [
      group("shortcuts.groupComparison", "shortcuts.contextComparison", [
        ["0–9/A–Z", "shortcuts.comparisonToggleAssignedMark"],
        ["Click", "shortcuts.comparisonPickWithoutKeep"],
        [mod + "+Click", "shortcuts.comparisonToggleKeepMark"],
        ["Shift+Click", "shortcuts.comparisonExtendMarkedRange"],
        ["Space", "shortcuts.comparisonOpenPickedLarger"],
        ["Arrows", "shortcuts.comparisonMovePicked"],
        ["Home/End", "shortcuts.comparisonFirstLastOnPage"],
        ["Shift+Arrows/Home/End", "shortcuts.comparisonExtendMarkedRange"],
        ["PageUp/PageDown", "shortcuts.comparisonBrowseUndecided"],
        [mod + "+A", "shortcuts.comparisonMarkPage"],
        ["Enter", "shortcuts.comparisonEnter"],
        ["Shift+Enter", "shortcuts.comparisonShiftEnter"],
        ["Delete/Backspace", "shortcuts.comparisonTrashMarked"],
        ["Shift+Delete/Backspace", "shortcuts.comparisonReviewPermanentMarked"],
        ["Double-click", "shortcuts.comparisonPickForInspection"],
        ["Escape", "shortcuts.comparisonLeaveWithoutApplying"],
      ]),
      group("shortcuts.groupApp", "shortcuts.contextApp", [
        [mod + "+R", "shortcuts.appRecheckSection"],
        [mod + "+Comma", "settings.title"],
        [mod + "+Slash / Question", "shortcuts.appShortcuts"],
        [mod + "+Equal/Plus/Semicolon", "app.zoomIn"],
        [mod + "+Minus", "app.zoomOut"],
        [mod + "+0", "shortcuts.appResetZoom"],
      ]),
    ],
  ];
}

export function shortcutGroups(): ShortcutGroup[] { return shortcutColumns().flat(); }
