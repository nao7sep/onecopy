import { primaryModWord } from "../utils/shortcuts";

export interface ShortcutRow { chord: string; action: string }
export interface ShortcutGroup { title: string; context: string; rows: ShortcutRow[] }

/** Semantic reading columns: Main navigation, viewing, then decisions/app.
 * Narrow layouts retain that DOM order without splitting a group. */
export function shortcutColumns(): ShortcutGroup[][] {
  const mod = primaryModWord();
  const group = (title: string, context: string, rows: [string, string][]): ShortcutGroup =>
    ({ title, context, rows: rows.map(([chord, action]) => ({ chord, action })) });
  return [
    [
      group("Main items", "item area focused; all file types", [
        ["Arrows", "Move selection; Other files uses Up/Down"],
        ["Home/End", "First/last item"],
        ["PageUp/PageDown", "Move by a screenful"],
        ["Shift+Arrows/Home/End/PageUp/PageDown", "Extend the selection range"],
        ["Click", "Select only this item"],
        [mod + "+Click", "Toggle this item's selection"],
        ["Shift+Click", "Extend the selection range"],
        ["Space", "Open Quick View"],
        ["Double-click", "Select this item and open Quick View"],
        ["Enter", "Compare images; play/pause an already visible video or audio player"],
        ["F", "Open fullscreen"],
        ["Delete/Backspace", "Trash selected items and all their copies"],
        ["Shift+Delete/Backspace", "Review permanent deletion of the selection"],
      ]),
      group("Sections", "Sections tree focused", [
        ["Up/Down", "Previous/next row; open a month"],
        ["PageUp/PageDown", "Move ten rows"],
        ["Home/End", "First/last row"],
        ["Left/Right", "Collapse/expand or go to parent/child"],
        ["Enter/Space", "Toggle a branch or open a month"],
        ["Right/Tab", "From a month, focus Main items"],
      ]),
      group("Destinations", "destination tree focused", [
        ["Up/Down", "Previous/next folder"],
        ["PageUp/PageDown", "Move ten folders"],
        ["Home/End", "First/last folder"],
        ["Left/Right", "Collapse/expand or go to parent/child"],
        ["Enter", "Expand/collapse only; use buttons to Copy or Move"],
      ]),
    ],
    [
      group("Quick View and fullscreen", "Main's temporary viewer; controls keep their own keys", [
        ["Space", "Quick View: return to Main; fullscreen: switch to Quick View"],
        ["F", "Quick View: switch to fullscreen; fullscreen: return to Main"],
        ["Escape", "Return to Main"],
        ["Left/Right", "Previous/next file"],
        ["PageUp/PageDown", "Previous/next image, video, or audio file"],
        ["Home/End", "First/last image, video, or audio file"],
        ["Delete/Backspace", "Trash only the displayed item and its copies"],
        ["Shift+Delete/Backspace", "Review permanent deletion of the displayed item"],
      ]),
      group("Preview window", "separate live Preview focused; controls and text keep their own keys", [
        ["F", "Toggle fullscreen on this live Preview"],
        ["Escape", "Leave fullscreen; otherwise close Preview"],
        ["Arrows/Home/End/PageUp/PageDown", "Navigate Main; Shift extends its selection"],
        ["Delete/Backspace", "Trash Main's complete selection"],
        ["Shift+Delete/Backspace", "Review permanent deletion of Main's selection"],
      ]),
      group("Media and text", "visible player or focused read-only document", [
        ["Enter", "Play/pause media; a focused timestamp seeks and plays"],
        ["Up/Down/PageUp/PageDown/Home/End", "Scroll focused text, attributes, or transcript"],
        [mod + "+A/C", "Select all/copy focused text"],
        ["Press and hold", "Inspect original image pixels or the paused video frame"],
      ]),
      group("Confirmations", "deletion review; Cancel starts focused", [
        ["Left/Right", "Focus the adjacent footer action"],
        ["Tab", "Move from Cancel to Delete"],
        ["Enter", "Activate the focused action"],
        ["Escape", "Cancel"],
      ]),
    ],
    [
      group("Comparison", "Comparison item area; marks apply only to this page", [
        ["0–9/A–Z", "Toggle the assigned image's keep mark (including F)"],
        ["Click", "Pick an image without changing Keep"],
        [mod + "+Click", "Toggle this image's keep mark"],
        ["Shift+Click", "Extend the marked range"],
        ["Space", "Open the picked image larger; Space/Escape returns"],
        ["Arrows", "Move the picked image spatially"],
        ["Home/End", "Pick the first/last image on this page"],
        ["Shift+Arrows/Home/End", "Extend the marked range"],
        ["PageUp/PageDown", "Browse undecided pages"],
        [mod + "+A", "Mark the current page to keep"],
        ["Enter", "No marks: close; with marks: review trashing the unmarked images"],
        ["Shift+Enter", "With marks: review permanently deleting the unmarked images"],
        ["Delete/Backspace", "Trash the marked images themselves"],
        ["Shift+Delete/Backspace", "Review permanently deleting the marked images"],
        ["Double-click", "Pick this image for inspection"],
        ["Escape", "Leave without applying marks"],
      ]),
      group("App", "Main window; not while a dialog or menu owns input", [
        [mod + "+R", "Recheck the section, outside Comparison/viewers and text editing"],
        [mod + "+Comma", "Settings"],
        [mod + "+Slash / Question", "Keyboard shortcuts; bare Question is inactive in text fields"],
        [mod + "+Equal/Plus/Semicolon", "Zoom in"],
        [mod + "+Minus", "Zoom out"],
        [mod + "+0", "Reset zoom"],
      ]),
    ],
  ];
}

export function shortcutGroups(): ShortcutGroup[] { return shortcutColumns().flat(); }
