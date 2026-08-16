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
  rows: ShortcutRow[];
}

export function shortcutGroups(): ShortcutGroup[] {
  const mod = primaryModWord();
  return [
    {
      title: "Browsing",
      rows: [
        { chord: "Arrows", action: "Move the selection" },
        { chord: "Home / End", action: "First or last item" },
        { chord: "Page Up / Page Down", action: "Move by a screenful" },
        { chord: "Shift+Arrows", action: "Extend the selection" },
        { chord: `${mod}+Click`, action: "Add or remove one item" },
        { chord: "Shift+Click", action: "Select a range" },
      ],
    },
    {
      title: "Looking",
      rows: [
        { chord: "Space", action: "Show or hide the preview (plays/pauses a loaded video)" },
        { chord: "P", action: "Show or hide the preview" },
        { chord: "Enter", action: "Go deeper: compare similar photos, video scenes, or 100% view" },
        { chord: "Z", action: "100% view of the original (in the preview)" },
        { chord: "F", action: "Fullscreen (in the preview window)" },
        { chord: "Escape", action: "Leave fullscreen, or close the preview window" },
      ],
    },
    {
      title: "Culling",
      rows: [
        { chord: "Delete / Backspace", action: "Trash the item and every copy" },
        { chord: "Shift+Delete", action: "Delete permanently, after confirming" },
      ],
    },
    {
      title: "Comparison view",
      rows: [
        { chord: "1–9 / 0 / A–F", action: "Keep the photo in that slot" },
        { chord: "Shift+1–9/0/A–F", action: "Not similar — remove that slot from the set (never deletes)" },
        { chord: "Enter", action: "Commit the turn" },
        { chord: "Shift+Enter", action: "Commit, deleting the rest permanently" },
        { chord: "Double-click", action: "Enlarge one slot" },
        { chord: "Escape", action: "Leave without committing" },
      ],
    },
    {
      title: "Destinations tree",
      rows: [
        { chord: "Enter", action: "Move the selection here, trash the other copies" },
        { chord: "Shift+Enter", action: "Move here, permanently delete the rest" },
        { chord: `${mod}+Enter`, action: "Copy here, leave everything in place" },
      ],
    },
    {
      title: "App",
      rows: [
        { chord: `${mod}+Comma`, action: "Settings" },
        { chord: `${mod}+Slash / Question`, action: "This help" },
        { chord: `${mod}+Equal/Plus/Semicolon`, action: "Zoom in" },
        { chord: `${mod}+Minus`, action: "Zoom out" },
        { chord: `${mod}+0`, action: "Reset zoom" },
      ],
    },
  ];
}
