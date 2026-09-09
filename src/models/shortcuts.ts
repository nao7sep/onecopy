// The shortcuts help surface's content, as DATA rather than markup.
//
// A hand-maintained help list drifts silently: nothing connects a printed
// chord to a live binding, so a row can outlive the key it describes and read
// as a bug in the app rather than in the list. Holding the rows here means the
// suite can walk them, and it is what makes "does this key actually work?" a
// question with an answer.
//
// Chords follow the keyboard-shortcut-conventions: spelled-out key names, `+`
// between a modifier and a key, `/` between alternatives that share one, and
// the RUNNING platform's modifier word (both Cmd and Ctrl always fire).

import { primaryModWord } from "../utils/shortcuts";

export interface ShortcutRow {
  chord: string;
  action: string;
}

export interface ShortcutGroup {
  title: string;
  /** Where these chords fire — every group is scoped to a focused surface,
   * and the unstated scope was the one gap in an otherwise-pinned catalogue:
   * a chord pressed with the wrong surface focused looks broken, not scoped. */
  context: string;
  rows: ShortcutRow[];
}

export function shortcutGroups(): ShortcutGroup[] {
  const mod = primaryModWord();
  return [
    {
      title: "Browsing",
      context: "when the photo grid has focus",
      rows: [
        { chord: "Arrows", action: "Move the selection" },
        { chord: "Home / End", action: "First or last item" },
        { chord: "Page Up / Page Down", action: "Move by a screenful" },
        { chord: "Shift+Arrows", action: "Extend the selection" },
        { chord: "Click", action: "Select only that item" },
        { chord: `${mod}+Click`, action: "Toggle one item in the selection" },
        { chord: "Shift+Click", action: "Select a range" },
      ],
    },
    {
      title: "Looking",
      context: "when Main's item area has focus",
      rows: [
        { chord: "Space", action: "Open Quick View" },
        { chord: "Enter", action: "Compare images or control media playback" },
        { chord: "F", action: "Open the selection in fullscreen" },
        {
          chord: "Escape",
          action: "Close the active viewer or preview window",
        },
      ],
    },
    {
      title: "Preview window",
      context: "while the separate live Preview has focus",
      rows: [
        { chord: "F", action: "Toggle full screen without leaving the live Preview" },
        { chord: "Escape", action: "Leave full screen; otherwise close Preview" },
      ],
    },
    {
      title: "Culling",
      context: "when the photo grid has focus",
      rows: [
        {
          chord: "Delete / Backspace",
          action: "Delete the item and every copy (recoverable)",
        },
        {
          chord: "Shift+Delete",
          action: "Delete permanently, after confirming",
        },
      ],
    },
    {
      title: "Comparison view",
      context: "while a comparison is open",
      rows: [
        {
          chord: "0–9 / A–Z",
          action: "Toggle that image's keep mark",
        },
        { chord: "Space", action: "Open the picked image in a larger window; Space returns" },
        { chord: "Arrows", action: "Move the active image spatially" },
        { chord: "Shift+Arrows", action: "Extend the marked range" },
        {
          chord: "Home / End",
          action: "Activate the first or last visible image",
        },
        { chord: "Page Up / Page Down", action: "Browse undecided pages" },
        { chord: `${mod}+A`, action: "Mark the current page to keep" },
        {
          chord: "Enter",
          action: "Close with no marks; otherwise review the visible decision",
        },
        {
          chord: "Shift+Enter",
          action:
            "Review permanently deleting the marked images' visible complement",
        },
        {
          chord: "Delete / Backspace",
          action: "Delete the marked images (recoverable)",
        },
        {
          chord: "Shift+Delete",
          action: "Permanently delete the marked images after confirming",
        },
        {
          chord: "Double-click",
          action: "Activate that image for inspection",
        },
        {
          chord: "Escape",
          action: "Leave without applying the keep marks",
        },
      ],
    },
    {
      title: "Confirmations",
      context: "while a deletion confirmation is open; Cancel starts focused",
      rows: [
        { chord: "Left / Right", action: "Focus the adjacent footer action" },
        { chord: "Tab", action: "Move from Cancel to Delete" },
        { chord: "Enter", action: "Activate the focused action" },
        { chord: "Escape", action: "Cancel" },
      ],
    },
    {
      title: "App",
      context: "anywhere",
      rows: [
        { chord: `${mod}+R`, action: "Recheck this section" },
        { chord: `${mod}+Comma`, action: "Settings" },
        { chord: `${mod}+Slash / Question`, action: "This help" },
        { chord: `${mod}+Equal/Plus/Semicolon`, action: "Zoom in" },
        { chord: `${mod}+Minus`, action: "Zoom out" },
        { chord: `${mod}+0`, action: "Reset zoom" },
      ],
    },
  ];
}
